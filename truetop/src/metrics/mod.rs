//! Per-process metrics: one module per metric, each owning its type, its
//! collection logic, and its tests. `backend` orchestrates them.
//!
//! Two shapes. Counter metrics (`cpu`, `iowait`) diff an eBPF nanosecond
//! counter between ticks via [`Snapshot`]; lookup metrics (`mem`, `identity`)
//! read per pid, for the visible rows only.

mod cpu;
mod identity;
mod iowait;
mod mem;

use std::{collections::HashMap, time::Instant};

pub(crate) use cpu::CpuCollector;
pub use cpu::CpuMetrics;
pub(crate) use identity::Resolver;
pub use iowait::IoMetrics;
pub(crate) use iowait::IoWaitCollector;
pub use mem::MemMetrics;
pub(crate) use mem::MemReader;

/// One process as the renderer sees it: identity plus one slot per subsystem.
/// Raw values only - the renderer formats them lazily for the visible rows
/// it draws.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProcessMetrics {
    pub pid: u32,
    pub name: String,
    pub user: Option<String>,
    pub cpu: CpuMetrics,
    pub mem: Option<MemMetrics>,
    pub io: Option<IoMetrics>,
}

/// A per-process counter snapshot at one instant. Counter metrics derive a
/// per-interval value from the delta between two snapshots over the elapsed
/// time.
#[derive(Default, Clone)]
pub(crate) struct Snapshot {
    pub(crate) at: Option<Instant>,
    pub(crate) by_pid: HashMap<u32, u64>,
}

impl Snapshot {
    pub(crate) fn new(by_pid: HashMap<u32, u64>) -> Self {
        Self {
            at: Some(Instant::now()),
            by_pid,
        }
    }

    /// Nanoseconds since `prev`, or 0 without a baseline (first tick).
    pub(crate) fn elapsed_ns_since(&self, prev: &Self) -> f64 {
        match (self.at, prev.at) {
            (Some(now), Some(was)) => now.duration_since(was).as_nanos() as f64,
            _ => 0.0,
        }
    }
}
