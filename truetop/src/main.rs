//! truetop — user-space entrypoint.
//!
//! This commit lands the **CO-RE foundation**: the eBPF program reads
//! `task_struct::pid` through an offset resolved at runtime against the live
//! kernel BTF and injected via a global (`btf`/`PID_OFFSET`), so one binary
//! works across kernel versions/arches. The data pipeline (per-CPU
//! accounting + delta math) still runs on **mock** data in `backend`.
//!
//! Topology:
//!   - one shared `ArcSwap<SystemState>` (the double buffer),
//!   - the collector runs as a Tokio task at 1 Hz (`backend`),
//!   - the renderer owns the main thread at ~60 fps (`ui`),
//!   - a SIGINT/SIGTERM listener flips a shared flag for graceful teardown.

mod backend;
mod btf;
mod ui;

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::Context as _;
use arc_swap::ArcSwap;
use aya::{EbpfLoader, maps::Array, programs::RawTracePoint};
use backend::SystemState;
use tokio::signal;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    // Bump the memlock rlimit before any BPF map allocation.
    // Harmless to set now; required once the loader is wired in. Older kernels
    // without memcg-based accounting need this, see https://lwn.net/Articles/837122/.
    let rlim = libc::rlimit {
        rlim_cur: libc::RLIM_INFINITY,
        rlim_max: libc::RLIM_INFINITY,
    };
    let ret = unsafe { libc::setrlimit(libc::RLIMIT_MEMLOCK, &rlim) };
    if ret != 0 {
        log::debug!("remove limit on locked memory failed, ret is: {ret}");
    }

    // --- CO-RE: resolve task_struct::pid against the running kernel's BTF and
    // inject it into the program before load. The same bytecode then reads the
    // field correctly on any kernel/arch.
    let pid_offset = btf::field_byte_offset("task_struct", "pid")
        .context("resolving task_struct::pid offset from kernel BTF")?;
    log::info!("CO-RE: task_struct::pid at byte offset {pid_offset} on this kernel");

    let mut ebpf = EbpfLoader::new()
        .override_global("PID_OFFSET", &pid_offset, true)
        .load(aya::include_bytes_aligned!(concat!(
            env!("OUT_DIR"),
            "/truetop"
        )))
        .context("loading eBPF object")?;

    let program: &mut RawTracePoint = ebpf
        .program_mut("sched_switch")
        .context("sched_switch program not found in object")?
        .try_into()?;
    program.load().context("loading sched_switch program")?;
    program
        .attach("sched_switch")
        .context("attaching sched_switch raw tracepoint")?;

    // Prove the injected offset reads real pids at runtime.
    {
        let last_pid: Array<_, u32> =
            Array::try_from(ebpf.map("LAST_PID").context("LAST_PID map not found")?)?;
        std::thread::sleep(Duration::from_millis(200));
        match last_pid.get(&0, 0) {
            Ok(pid) => log::info!("CO-RE smoke test: sample scheduled-in pid = {pid}"),
            Err(e) => log::warn!("CO-RE smoke test: could not read LAST_PID: {e}"),
        }
    }

    let shared = Arc::new(ArcSwap::from_pointee(SystemState::default()));

    let running = Arc::new(AtomicBool::new(true));

    let collector = tokio::spawn(backend::collector_loop(Arc::clone(&shared)));

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

    render_result.map_err(Into::into)
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
