//! truetop - eBPF-backed per-process CPU/memory collection.
//!
//! The eBPF program reads `task_struct` fields through offsets resolved at
//! runtime against the live kernel BTF and injected as globals (`btf`), so one
//! binary works across kernel versions/arches. It accumulates per-CPU on-CPU
//! nanoseconds; `backend` derives utilisation and `ui` renders it.
//!
//! The binary is a thin shell over [`run`]; integration tests drive
//! [`load_ebpf`]/[`setup_collector`] and [`Collector`] directly.

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
    EbpfLoader,
    maps::{HashMap, Map, PerCpuArray, RingBuf},
    programs::RawTracePoint,
    util::nr_cpus,
};
pub use backend::{Collector, SystemState};
// Exposed for the batch integration test, not part of the stable API.
#[doc(hidden)]
pub use batch::BatchReader;
use cpu_maps::CpuMaps;
use tokio::signal;
use truetop_common::COMM_LEN;

/// Load and attach the eBPF, then run either the headless bench loop or the UI.
pub async fn run() -> anyhow::Result<()> {
    let ticks = match cli::parse(std::env::args().skip(1))? {
        cli::Command::Print(text) => {
            println!("{text}");
            return Ok(());
        }
        cli::Command::Bench(ticks) => Some(ticks),
        cli::Command::Ui => None,
    };

    env_logger::init();
    raise_memlock();

    let (ebpf, collector) = attach().map_err(diagnose)?;

    match ticks {
        Some(ticks) => backend::run_headless(collector, ticks),
        None => run_ui(collector).await?,
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
    let mut ebpf = load_ebpf()?;
    let collector = setup_collector(&mut ebpf)?;
    Ok((ebpf, collector))
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
fn load_ebpf() -> anyhow::Result<aya::Ebpf> {
    let pid = btf::field_byte_offset("task_struct", "pid").context("BTF: task_struct::pid")?;
    let tgid = btf::field_byte_offset("task_struct", "tgid").context("BTF: task_struct::tgid")?;
    // The sched_switch hotpath reads pid and tgid in one 8-byte load
    // (`task::pid_and_tgid`), which requires them adjacent.
    anyhow::ensure!(
        tgid == pid + 4,
        "task_struct pid ({pid}) and tgid ({tgid}) are not adjacent in this kernel"
    );
    let state = state_offset()?;
    // A miss falls back to the probe-read path, correct on any kernel.
    let has_prev_state = btf::sched_switch_has_prev_state().unwrap_or(false);
    log::info!(
        "CO-RE offsets - pid: {pid}, tgid: {tgid}, state: {state}; prev_state arg: {has_prev_state}"
    );

    EbpfLoader::new()
        .override_global("PID_OFFSET", &pid, true)
        .override_global("TGID_OFFSET", &tgid, true)
        .override_global("STATE_OFFSET", &state, true)
        .override_global("HAS_PREV_STATE", &u32::from(has_prev_state), true)
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
fn setup_collector(ebpf: &mut aya::Ebpf) -> anyhow::Result<Collector> {
    for tp in [
        "sched_switch",
        "sched_process_exec",
        "sched_process_fork",
        "sched_process_exit",
    ] {
        attach_raw_tracepoint(ebpf, tp)?;
    }

    // Counter maps stay raw MapData so the collector can do
    // BPF_MAP_LOOKUP_BATCH (aya exposes no batch API; see `batch`).
    let cpu_ns = take_percpu_map(ebpf, "CPU_NS")?;
    let iowait_ns = take_percpu_map(ebpf, "IOWAIT_NS")?;
    let comm: HashMap<_, u32, [u8; COMM_LEN]> =
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
    let ncpus = nr_cpus().map_err(|(s, e)| anyhow::anyhow!("{s}: {e}"))?;

    let cpu_maps = CpuMaps::new(cpu_ns, start_time, current_tgid);
    Ok(Collector::new(cpu_maps, iowait_ns, comm, exits, ncpus))
}

fn take_percpu_map(ebpf: &mut aya::Ebpf, name: &str) -> anyhow::Result<aya::maps::MapData> {
    let Map::PerCpuHashMap(map) = ebpf
        .take_map(name)
        .with_context(|| format!("{name} map not found"))?
    else {
        anyhow::bail!("{name} is not a per-CPU hash map");
    };
    Ok(map)
}

/// Renderer on the main thread, collector on a 1 Hz Tokio task, plus a
/// SIGINT/SIGTERM listener - until the user quits.
async fn run_ui(collector: Collector) -> anyhow::Result<()> {
    let shared = Arc::new(ArcSwap::from_pointee(SystemState::default()));
    let running = Arc::new(AtomicBool::new(true));

    let collector_task = tokio::spawn(backend::collector_loop(Arc::clone(&shared), collector));
    let signal_running = Arc::clone(&running);
    let signal_task = tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        signal_running.store(false, Ordering::Relaxed);
    });

    let result = ui::render_app(Arc::clone(&shared), Arc::clone(&running));

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

/// Resolve when the process receives SIGINT or (on Unix) SIGTERM.
async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal as unix_signal};
        let mut term = match unix_signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => {
                let _ = signal::ctrl_c().await;
                return;
            }
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

    use super::{PRIVILEGE_HINT, privilege_hint};

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
