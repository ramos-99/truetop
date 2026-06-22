//! truetop — user-space entrypoint.
//!
//! The eBPF program reads `task_struct` fields through offsets resolved at
//! runtime against the live kernel BTF and injected as globals (`btf`), so one
//! binary works across kernel versions/arches. It accumulates per-CPU on-CPU
//! nanoseconds; `backend` derives utilisation and `ui` renders it.

mod backend;
mod batch;
mod btf;
mod metrics;
mod ui;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use anyhow::Context as _;
use arc_swap::ArcSwap;
use aya::{
    EbpfLoader,
    maps::{HashMap, Map},
    programs::RawTracePoint,
    util::nr_cpus,
};
use backend::{Collector, SystemState};
use tokio::signal;
use truetop_common::COMM_LEN;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    raise_memlock();

    let mut ebpf = load_ebpf()?;
    let collector = setup_collector(&mut ebpf)?;

    match bench_ticks() {
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

/// Load the eBPF object, resolving task_struct field offsets from the live
/// kernel BTF and injecting them as globals — our portable CO-RE (see `btf`).
fn load_ebpf() -> anyhow::Result<aya::Ebpf> {
    let pid = btf::field_byte_offset("task_struct", "pid").context("BTF: task_struct::pid")?;
    let tgid = btf::field_byte_offset("task_struct", "tgid").context("BTF: task_struct::tgid")?;
    log::info!("CO-RE offsets — pid: {pid}, tgid: {tgid}");

    EbpfLoader::new()
        .override_global("PID_OFFSET", &pid, true)
        .override_global("TGID_OFFSET", &tgid, true)
        .load(aya::include_bytes_aligned!(concat!(
            env!("OUT_DIR"),
            "/truetop"
        )))
        .context("loading eBPF object")
}

/// Attach the tracepoints and build a [`Collector`] over the CPU and comm maps.
/// `ebpf` must outlive the collector — it owns the tracepoint links.
fn setup_collector(ebpf: &mut aya::Ebpf) -> anyhow::Result<Collector> {
    for tp in ["sched_switch", "sched_process_exec", "sched_process_exit"] {
        attach_raw_tracepoint(ebpf, tp)?;
    }

    // CPU_NS stays raw MapData so the collector can do BPF_MAP_LOOKUP_BATCH (aya
    // exposes no batch API; see `batch`).
    let Map::PerCpuHashMap(cpu_ns) = ebpf.take_map("CPU_NS").context("CPU_NS map not found")?
    else {
        anyhow::bail!("CPU_NS is not a per-CPU hash map");
    };
    let comm: HashMap<_, u32, [u8; COMM_LEN]> =
        HashMap::try_from(ebpf.take_map("COMM_MAP").context("COMM_MAP not found")?)?;
    let ncpus = nr_cpus().map_err(|(s, e)| anyhow::anyhow!("{s}: {e}"))?;

    Ok(Collector::new(cpu_ns, comm, ncpus))
}

/// Renderer on the main thread, collector on a 1 Hz Tokio task, plus a
/// SIGINT/SIGTERM listener — until the user quits.
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

/// `--bench <TICKS>`: headless mode running `TICKS` collector ticks, no UI.
fn bench_ticks() -> Option<u32> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--bench" {
            return args.next()?.parse().ok();
        }
    }
    None
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
