//! Collector thread (backend).
//!
//! Owns the data-production side of the double-buffer described in
//! `CLAUDE.md` §3. It wakes on a fixed 1000 ms interval, builds a fresh
//! [`SystemState`] snapshot, and publishes it via an atomic pointer swap.
//!
//! Phase 1 skeleton: the snapshot is filled with mock data. The real
//! implementation will replace [`collect_mock`] with a `bpf_map_lookup_batch`
//! pull + per-CPU delta aggregation, but the publish/consume contract with the
//! renderer stays identical so the UI never has to change.

use std::{sync::Arc, time::Duration};

use arc_swap::ArcSwap;
use tokio::time::{MissedTickBehavior, interval};

/// Per-process metrics as consumed by the renderer.
///
/// This is intentionally a *user-space* aggregate, not an FFI struct: it holds
/// already-computed percentages and totals. The `#[repr(C)]` counter structs
/// that cross the eBPF boundary live in `truetop-common` and get reduced down
/// into this shape by the collector.
#[derive(Debug, Clone)]
pub struct ProcessData {
    pub pid: u32,
    pub cpu_percent: f32,
    pub memory_bytes: u64,
}

/// An immutable snapshot of the whole system at one collector tick.
///
/// The renderer only ever sees this through a shared `Arc`, so it is always
/// observed as a consistent, complete picture — never a half-written buffer.
#[derive(Debug, Clone, Default)]
pub struct SystemState {
    /// Monotonic collector tick counter. Lets the UI (and humans) confirm the
    /// backend is advancing independently of the render rate.
    pub tick: u64,
    pub processes: Vec<ProcessData>,
}

/// Build one mock snapshot. Stands in for the eBPF batch pull until Phase 1
/// wiring lands. Values shift with `tick` so the decoupled refresh is visible.
fn collect_mock(tick: u64) -> SystemState {
    let processes = (0..6)
        .map(|i| {
            let pid = 1000 + i as u32;
            // Deterministic but tick-varying so the table animates at 1 Hz.
            let phase = (tick.wrapping_add(i)) as f32;
            ProcessData {
                pid,
                cpu_percent: (phase * 0.7).sin().abs() * 100.0,
                memory_bytes: (8 + i) * 1024 * 1024 + (tick % 32) * 256 * 1024,
            }
        })
        .collect();

    SystemState { tick, processes }
}

/// Drive the backend double-buffer.
///
/// Wakes every 1000 ms, builds a new [`SystemState`], and `store`s it into the
/// shared `ArcSwap`. The previous snapshot is dropped automatically once the
/// last reader releases it. Runs until the task is dropped at runtime shutdown.
pub async fn collector_loop(shared: Arc<ArcSwap<SystemState>>) {
    let mut ticker = interval(Duration::from_millis(1000));
    // If a tick is missed (e.g. blocked elsewhere) we don't want a burst of
    // catch-up ticks; just resume on the regular cadence.
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    let mut tick: u64 = 0;
    loop {
        ticker.tick().await;
        tick = tick.wrapping_add(1);
        shared.store(Arc::new(collect_mock(tick)));
    }
}
