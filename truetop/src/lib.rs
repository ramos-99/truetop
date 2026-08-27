//! truetop - eBPF-backed per-process CPU/memory collection.
//!
//! The eBPF program reads `task_struct` fields through offsets resolved at
//! runtime against the live kernel BTF and injected as globals (`btf`), so one
//! binary works across kernel versions/arches. It accumulates per-CPU on-CPU
//! nanoseconds; `backend` derives utilisation and `ui` renders it.
//!
//! The binary is a thin shell over [`run`]; integration tests drive
//! [`load_ebpf`]/[`setup_collector`] and [`Collector`] directly.
//!
//! - [`backend`]   - the collector: ticks the maps into a [`SystemState`] snapshot,
//! - [`batch`]     - the raw `BPF_MAP_LOOKUP_BATCH` / delete syscalls aya has no typed API for,
//! - [`btf`]       - resolves a kernel struct field's byte offset from live BTF,
//! - [`cli`]       - argument parsing, before anything privileged happens,
//! - [`cpu_maps`]  - the CPU-side kernel maps: reads, in-flight correction, deletion,
//! - [`metrics`]   - one module per metric (`cpu`, `iowait`, `mem`, `identity`),
//! - [`reaper`]    - deletes a departed process's map entries, budgeted per tick,
//! - [`system`]    - host facts (`Machine`) and the CPU topology (`CpuCounts`),
//! - [`ui`]        - the renderer: sorts, formats and draws; never collects, never blocks.

mod backend;
mod batch;
mod btf;
mod cli;
mod cpu_maps;
pub mod metrics;
mod reaper;
mod system;
mod ui;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use anyhow::Context as _;
use arc_swap::ArcSwap;
use aya::{
    Btf, EbpfLoader,
    maps::{HashMap, Map, PerCpuArray, RingBuf},
    programs::{FEntry, RawTracePoint},
};
pub use backend::{Collector, SystemState};
// Exposed for the batch integration test, not part of the stable API.
#[doc(hidden)]
pub use batch::BatchReader;
use cpu_maps::CpuMaps;
use system::{CpuCounts, Machine};
use tokio::signal;
use truetop_common::{COMM_LEN, Identity};

/// Load and attach the eBPF, then run either the headless bench loop or the UI.
pub async fn run() -> anyhow::Result<()> {
    let args = cli::parse(std::env::args().skip(1))?;
    let ticks = match args.command {
        cli::Command::Print(text) => {
            println!("{text}");
            return Ok(());
        }
        cli::Command::Bench(ticks) => Some(ticks),
        cli::Command::Ui => None,
    };

    env_logger::init();
    raise_memlock();

    let cpus = CpuCounts::detect()?;
    log::info!("CPUs - {} possible, {} online", cpus.possible, cpus.online);
    let (ebpf, collector) = attach_with(cpus, args.max_processes).map_err(diagnose)?;

    match ticks {
        Some(ticks) => backend::run_headless(collector, ticks),
        None => run_ui(collector, cpus, ui::theme::resolve(args.background)).await?,
    }

    // `ebpf` owns the tracepoint links; dropping it here detaches them.
    drop(ebpf);
    Ok(())
}

/// Raise the memlock rlimit before the loader allocates any BPF map; kernels
/// without memcg-based accounting need it (https://lwn.net/Articles/837122/).
fn raise_memlock() {
    let rlim = libc::rlimit {
        rlim_cur: libc::RLIM_INFINITY,
        rlim_max: libc::RLIM_INFINITY,
    };
    // SAFETY: a valid resource id and an initialised rlimit.
    if unsafe { libc::setrlimit(libc::RLIMIT_MEMLOCK, &rlim) } != 0 {
        log::debug!("could not raise RLIMIT_MEMLOCK");
    }
}

/// Load the eBPF, attach the tracepoints, and build the collector. The returned
/// [`aya::Ebpf`] owns the tracepoint links - keep it alive for the collector's
/// lifetime.
pub fn attach() -> anyhow::Result<(aya::Ebpf, Collector)> {
    attach_with(CpuCounts::detect()?, None)
}

/// [`attach`] with the accounting maps sized to `max_processes`, as
/// `--max-processes` sizes them.
#[doc(hidden)]
pub fn attach_with_capacity(max_processes: u32) -> anyhow::Result<(aya::Ebpf, Collector)> {
    attach_with(CpuCounts::detect()?, Some(max_processes))
}

/// [`attach`] over an already-resolved topology, so one process reads it once.
fn attach_with(
    cpus: CpuCounts,
    max_processes: Option<u32>,
) -> anyhow::Result<(aya::Ebpf, Collector)> {
    let entries = max_processes.unwrap_or_else(|| default_max_processes(cpus.possible));
    log::info!(
        "accounting maps: {entries} entries, ~{} MiB of kernel memory",
        map_bytes(entries, cpus.possible) >> 20
    );

    let mut ebpf = load_ebpf(entries)?;
    let collector = setup_collector(&mut ebpf, cpus)?;
    Ok((ebpf, collector))
}

/// Entries per accounting map when the operator has not chosen. Capped by a
/// memory budget rather than flat, because an LRU map is preallocated and a
/// per-CPU one costs `entries × 8 × possible_cpus`.
fn default_max_processes(possible_cpus: usize) -> u32 {
    const CEILING: u32 = 65_536;
    const FLOOR: u32 = 16_384;
    const BUDGET_PER_MAP: u64 = 32 << 20;

    let per_entry = 8 * possible_cpus.max(1) as u64;
    let affordable = u32::try_from(BUDGET_PER_MAP / per_entry).unwrap_or(u32::MAX);
    affordable.clamp(FLOOR, CEILING)
}

/// What those maps commit at load: two per-CPU counters, the name map, and the
/// tid → timestamp map (a `u32` key and a `u64` value).
fn map_bytes(entries: u32, possible_cpus: usize) -> u64 {
    let entries = u64::from(entries);
    2 * entries * 8 * possible_cpus as u64 + entries * COMM_LEN as u64 + entries * 12
}

const PRIVILEGE_HINT: &str = "truetop loads eBPF, which needs privilege: run it as root (sudo truetop), or \
     grant the binary CAP_BPF and CAP_PERFMON";

/// Prepend an actionable hint to an attach failure the user is likely to hit;
/// anything unrecognised keeps its own error chain untouched.
fn diagnose(err: anyhow::Error) -> anyhow::Error {
    match privilege_hint(&err) {
        Some(hint) => err.context(hint),
        None => err,
    }
}

/// The privilege hint when the failure is a permission error anywhere in the
/// chain (aya surfaces the load EPERM as an `io::Error` source), else `None`.
fn privilege_hint(err: &anyhow::Error) -> Option<&'static str> {
    err.chain()
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
        .find_map(|io| {
            (io.kind() == std::io::ErrorKind::PermissionDenied).then_some(PRIVILEGE_HINT)
        })
}

/// Load the eBPF object, resolving task_struct field offsets from the live
/// kernel BTF and injecting them as globals - our portable CO-RE (see `btf`).
fn load_ebpf(max_processes: u32) -> anyhow::Result<aya::Ebpf> {
    let pid = btf::field_byte_offset("task_struct", "pid").context("BTF: task_struct::pid")?;
    let tgid = btf::field_byte_offset("task_struct", "tgid").context("BTF: task_struct::tgid")?;
    // The sched_switch hotpath reads pid and tgid in one 8-byte load
    // (`task::pid_and_tgid`), which requires them adjacent.
    anyhow::ensure!(
        tgid == pid + 4,
        "task_struct pid ({pid}) and tgid ({tgid}) are not adjacent in this kernel"
    );
    // Live-thread count of the group, read on the exit path to tell a departed
    // process from a leader that exited while its threads ran on.
    let signal =
        btf::field_byte_offset("task_struct", "signal").context("BTF: task_struct::signal")?;
    let live =
        btf::field_byte_offset("signal_struct", "live").context("BTF: signal_struct::live")?;
    let cred_uid = btf::field_byte_offset("cred", "uid").context("BTF: cred::uid")?;
    let state = state_offset()?;
    // A miss falls back to the probe-read path, correct on any kernel.
    let has_prev_state = btf::sched_switch_has_prev_state().unwrap_or(false);
    log::info!(
        "CO-RE offsets - pid: {pid}, tgid: {tgid}, state: {state}, signal: {signal}, live: {live}; \
         prev_state arg: {has_prev_state}"
    );

    EbpfLoader::new()
        .override_global("PID_OFFSET", &pid, true)
        .override_global("TGID_OFFSET", &tgid, true)
        .override_global("STATE_OFFSET", &state, true)
        .override_global("SIGNAL_OFFSET", &signal, true)
        .override_global("LIVE_OFFSET", &live, true)
        .override_global("CRED_UID_OFFSET", &cred_uid, true)
        .override_global("HAS_PREV_STATE", &u32::from(has_prev_state), true)
        // The accounting maps, all sized by `--max-processes`.
        .map_max_entries("CPU_NS", max_processes)
        .map_max_entries("IOWAIT_NS", max_processes)
        .map_max_entries("COMM_MAP", max_processes)
        .map_max_entries("SLEEP_SINCE", max_processes)
        .load(aya::include_bytes_aligned!(concat!(
            env!("OUT_DIR"),
            "/truetop"
        )))
        .context("loading eBPF object")
}

/// Offset of the run state: `__state` (u32) since 5.14, previously `state` - a
/// native long, whose meaningful low word sits in the second half on 64-bit
/// big-endian.
fn state_offset() -> anyhow::Result<u32> {
    if let Ok(offset) = btf::field_byte_offset("task_struct", "__state") {
        return Ok(offset);
    }
    let offset =
        btf::field_byte_offset("task_struct", "state").context("BTF: task_struct::state")?;
    let wide_be = cfg!(target_endian = "big") && cfg!(target_pointer_width = "64");
    Ok(offset + if wide_be { 4 } else { 0 })
}

/// Attach the tracepoints and build a [`Collector`] over the CPU and comm maps.
/// `ebpf` must outlive the collector - it owns the tracepoint links.
fn setup_collector(ebpf: &mut aya::Ebpf, cpus: CpuCounts) -> anyhow::Result<Collector> {
    for tp in [
        "sched_switch",
        "sched_process_exec",
        "sched_process_fork",
        "sched_process_exit",
    ] {
        attach_raw_tracepoint(ebpf, tp)?;
    }
    // Accuracy, not function: without it a process that drops privileges after
    // exec keeps the uid it was born with.
    if let Err(err) = attach_creds(ebpf) {
        log::warn!("credential changes will go unnoticed: {err:#}");
    }

    // Counter maps stay raw MapData so the collector can do
    // BPF_MAP_LOOKUP_BATCH (aya exposes no batch API; see `batch`).
    let cpu_ns = take_percpu_map(ebpf, "CPU_NS")?;
    let iowait_ns = take_percpu_map(ebpf, "IOWAIT_NS")?;
    let comm: HashMap<_, u32, Identity> =
        HashMap::try_from(ebpf.take_map("COMM_MAP").context("COMM_MAP not found")?)?;
    let exits: RingBuf<_> =
        RingBuf::try_from(ebpf.take_map("EXITS").context("EXITS map not found")?)?;
    let start_time: PerCpuArray<_, u64> = PerCpuArray::try_from(
        ebpf.take_map("START_TIME")
            .context("START_TIME map not found")?,
    )?;
    let current_tgid: PerCpuArray<_, u32> = PerCpuArray::try_from(
        ebpf.take_map("CURRENT_TGID")
            .context("CURRENT_TGID map not found")?,
    )?;
    let cpu_maps = CpuMaps::new(cpu_ns, start_time, current_tgid);
    // The kernel's number, not the one we asked for: it is what saturation is
    // measured against.
    let (_, capacity) = cpu_maps.kind_and_capacity()?;
    Ok(Collector::new(
        cpu_maps, iowait_ns, comm, exits, cpus, capacity,
    ))
}

fn take_percpu_map(ebpf: &mut aya::Ebpf, name: &str) -> anyhow::Result<aya::maps::MapData> {
    match ebpf
        .take_map(name)
        .with_context(|| format!("{name} map not found"))?
    {
        Map::PerCpuHashMap(map) | Map::PerCpuLruHashMap(map) => Ok(map),
        _ => anyhow::bail!("{name} is not a per-CPU hash map"),
    }
}

/// Renderer on the main thread, collector on a 1 Hz Tokio task, plus a
/// SIGINT/SIGTERM listener - until the user quits.
async fn run_ui(
    collector: Collector,
    cpus: CpuCounts,
    background: ui::theme::Background,
) -> anyhow::Result<()> {
    let shared = Arc::new(ArcSwap::from_pointee(SystemState::default()));
    let running = Arc::new(AtomicBool::new(true));

    let collector_task = tokio::spawn(backend::collector_loop(Arc::clone(&shared), collector));
    let signal_running = Arc::clone(&running);
    let signal_task = tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        signal_running.store(false, Ordering::Relaxed);
    });

    let result = ui::render_app(
        Arc::clone(&shared),
        Arc::clone(&running),
        Machine::detect(cpus),
        background,
    );

    running.store(false, Ordering::Relaxed);
    collector_task.abort();
    signal_task.abort();
    result.map_err(Into::into)
}

/// Load and attach the raw tracepoint whose program and tracepoint share `name`.
fn attach_raw_tracepoint(ebpf: &mut aya::Ebpf, name: &'static str) -> anyhow::Result<()> {
    let program: &mut RawTracePoint = ebpf
        .program_mut(name)
        .with_context(|| format!("program `{name}` not found in object"))?
        .try_into()?;
    program
        .load()
        .with_context(|| format!("loading `{name}`"))?;
    program
        .attach(name)
        .with_context(|| format!("attaching `{name}`"))?;
    Ok(())
}

/// The one hook that is not a raw tracepoint, because the kernel exposes none
/// for credential changes. `fentry` costs a trampoline on a path that fires
/// about once per process lifetime.
fn attach_creds(ebpf: &mut aya::Ebpf) -> anyhow::Result<()> {
    let btf = Btf::from_sys_fs().context("reading kernel BTF")?;
    let program: &mut FEntry = ebpf
        .program_mut("commit_creds")
        .context("program `commit_creds` not found in object")?
        .try_into()?;
    program.load("commit_creds", &btf).context("loading")?;
    program.attach().context("attaching")?;
    Ok(())
}

/// Resolve when the process receives SIGINT or (on Unix) SIGTERM.
async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal as unix_signal};
        let Ok(mut term) = unix_signal(SignalKind::terminate()) else {
            let _ = signal::ctrl_c().await;
            return;
        };
        tokio::select! {
            _ = signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        let _ = signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Error, ErrorKind};

    use super::{PRIVILEGE_HINT, default_max_processes, privilege_hint};

    #[test]
    fn the_default_map_size_tracks_the_cpu_count() {
        assert_eq!(default_max_processes(16), 65_536);
        assert_eq!(default_max_processes(128), 32_768);
        assert_eq!(default_max_processes(4096), 16_384);
    }

    #[test]
    fn permission_denied_anywhere_in_the_chain_hints_privilege() {
        // aya surfaces a load EPERM as an io::Error source under its own context.
        let err = anyhow::Error::new(Error::from(ErrorKind::PermissionDenied))
            .context("loading eBPF object");
        assert_eq!(privilege_hint(&err), Some(PRIVILEGE_HINT));
    }

    #[test]
    fn unrelated_failures_are_left_alone() {
        assert_eq!(privilege_hint(&anyhow::anyhow!("COMM_MAP not found")), None);
        let missing = anyhow::Error::new(Error::from(ErrorKind::NotFound));
        assert_eq!(privilege_hint(&missing), None);
    }
}
