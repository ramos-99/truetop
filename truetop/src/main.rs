//! truetop — user-space entrypoint.
//!
//! Phase 1 skeleton (`CLAUDE.md` §3/§5): wires the double-buffer dual-thread
//! model with **mock** data. No eBPF loading or kernel hooks yet — those slot
//! in behind `backend::collector_loop` without changing this wiring or the UI.
//!
//! Topology:
//!   - one shared `ArcSwap<SystemState>` (the double buffer),
//!   - the collector runs as a Tokio task at 1 Hz (`backend`),
//!   - the renderer owns the main thread at ~60 fps (`ui`),
//!   - a SIGINT/SIGTERM listener flips a shared flag for graceful teardown.

mod backend;
mod ui;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use arc_swap::ArcSwap;
use backend::SystemState;
use tokio::signal;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    // Bump the memlock rlimit before any BPF map allocation (CLAUDE.md §4).
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

    // The double buffer. Seeded with an empty snapshot so the renderer always
    // has something consistent to load before the collector's first tick.
    let shared = Arc::new(ArcSwap::from_pointee(SystemState::default()));

    // Shared run flag: cleared by the UI on 'q' or by the signal listener.
    let running = Arc::new(AtomicBool::new(true));

    // Backend collector — independent cadence, never blocks the renderer.
    let collector = tokio::spawn(backend::collector_loop(Arc::clone(&shared)));

    // Graceful teardown: translate SIGINT/SIGTERM into a run-flag clear so the
    // render loop unwinds, restores the terminal, and lets us detach cleanly.
    let signal_running = Arc::clone(&running);
    let signal_task = tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        signal_running.store(false, Ordering::Relaxed);
    });

    // Renderer owns the main thread. crossterm's polled event loop blocks here;
    // the Tokio tasks above run on other runtime workers (rt-multi-thread).
    let render_result = ui::render_app(Arc::clone(&shared), Arc::clone(&running));

    // UI has exited (q or signal). Ensure peers stop and the flag is settled.
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
