//! truetop — user-space entrypoint.
//!
//! The eBPF program reads `task_struct::pid` through an offset resolved at
//! runtime against the live kernel BTF and injected via a global
//! (`btf`/`PID_OFFSET`), so one binary works across kernel versions/arches. It
//! accumulates per-CPU on-CPU nanoseconds; `backend` derives utilisation.
//!
//! Topology:
//!   - one shared `ArcSwap<SystemState>` (the double buffer),
//!   - the collector runs as a Tokio task at 1 Hz (`backend`),
//!   - the renderer owns the main thread at ~60 fps (`ui`),
//!   - a SIGINT/SIGTERM listener flips a shared flag for graceful teardown.

mod backend;
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
    maps::{HashMap, PerCpuHashMap},
    programs::RawTracePoint,
    util::nr_cpus,
};
use backend::{Collector, SystemState};
use tokio::signal;
use truetop_common::COMM_LEN;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    // Raise the memlock rlimit before the loader allocates any BPF map; kernels
    // without memcg-based accounting need it, see https://lwn.net/Articles/837122/.
    let rlim = libc::rlimit {
        rlim_cur: libc::RLIM_INFINITY,
        rlim_max: libc::RLIM_INFINITY,
    };
    let ret = unsafe { libc::setrlimit(libc::RLIMIT_MEMLOCK, &rlim) };
    if ret != 0 {
        log::debug!("remove limit on locked memory failed, ret is: {ret}");
    }

    let mut ebpf = load_ebpf()?;

    attach_raw_tracepoint(&mut ebpf, "sched_switch")?;
    attach_raw_tracepoint(&mut ebpf, "sched_process_exec")?;
    attach_raw_tracepoint(&mut ebpf, "sched_process_exit")?;

    let cpu_ns: PerCpuHashMap<_, u32, u64> =
        PerCpuHashMap::try_from(ebpf.take_map("CPU_NS").context("CPU_NS map not found")?)?;
    let comm: HashMap<_, u32, [u8; COMM_LEN]> =
        HashMap::try_from(ebpf.take_map("COMM_MAP").context("COMM_MAP not found")?)?;
    let ncpus = nr_cpus().map_err(|(s, e)| anyhow::anyhow!("{s}: {e}"))?;

    let shared = Arc::new(ArcSwap::from_pointee(SystemState::default()));

    let running = Arc::new(AtomicBool::new(true));

    let collector = tokio::spawn(backend::collector_loop(
        Arc::clone(&shared),
        Collector::new(cpu_ns, comm, ncpus),
    ));

    // Graceful teardown: translate SIGINT/SIGTERM into a run-flag clear so the
    // render loop unwinds, restores the terminal, and lets us detach cleanly.
    let signal_running = Arc::clone(&running);
    let signal_task = tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        signal_running.store(false, Ordering::Relaxed);
    });

    let render_result = ui::render_app(Arc::clone(&shared), Arc::clone(&running));

    running.store(false, Ordering::Relaxed);
    collector.abort();
    signal_task.abort();

    // `ebpf` owns the tracepoint links; dropping it here detaches them.
    drop(ebpf);
    render_result.map_err(Into::into)
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
