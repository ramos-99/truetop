//! Collector (backend) — producer side of the double-buffer (CLAUDE.md §3).
//! Wakes at 1 Hz, pulls the eBPF per-CPU CPU-time counters, turns cumulative
//! nanoseconds into a per-interval utilisation %, and publishes an immutable
//! snapshot via an atomic pointer swap.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use arc_swap::ArcSwap;
use aya::maps::{MapData, PerCpuHashMap};
use tokio::time::{MissedTickBehavior, interval};

use crate::metrics::{CpuMetrics, ProcessMetrics};

/// Most rows published per tick, sorted by CPU% descending.
const MAX_ROWS: usize = 256;

/// Immutable snapshot of the system at one tick; the renderer only ever sees
/// this through a shared `Arc`, never a half-written buffer.
#[derive(Debug, Clone, Default)]
pub struct SystemState {
    pub tick: u64,
    pub processes: Vec<ProcessMetrics>,
}

/// Owns the persistent collection state; each [`Collector::tick`] reads the
/// counters and produces the next snapshot.
pub struct Collector {
    cpu_ns: PerCpuHashMap<MapData, u32, u64>,
    ncpus: f64,
    prev: Totals,
    tick: u64,
}

impl Collector {
    pub fn new(cpu_ns: PerCpuHashMap<MapData, u32, u64>, ncpus: usize) -> Self {
        Self {
            cpu_ns,
            ncpus: ncpus.max(1) as f64,
            prev: Totals::default(),
            tick: 0,
        }
    }

    fn tick(&mut self) -> SystemState {
        self.tick = self.tick.wrapping_add(1);
        let current = Totals::read(&self.cpu_ns);
        let processes = current.utilisation_since(&self.prev, self.ncpus);
        self.prev = current;
        SystemState {
            tick: self.tick,
            processes,
        }
    }
}

/// Drive the double-buffer at 1 Hz until the task is dropped at shutdown.
pub async fn collector_loop(shared: Arc<ArcSwap<SystemState>>, mut collector: Collector) {
    let mut ticker = interval(Duration::from_millis(1000));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        shared.store(Arc::new(collector.tick()));
    }
}

/// Cumulative on-CPU nanoseconds per pid at one instant, plus when it was taken.
struct Totals {
    at: Instant,
    by_pid: HashMap<u32, u64>,
}

impl Default for Totals {
    fn default() -> Self {
        Self {
            at: Instant::now(),
            by_pid: HashMap::new(),
        }
    }
}

impl Totals {
    /// Snapshot the map, summing each pid's value across the per-CPU slots.
    fn read(cpu_ns: &PerCpuHashMap<MapData, u32, u64>) -> Self {
        let by_pid = cpu_ns
            .iter()
            .flatten()
            .map(|(pid, per_cpu)| (pid, per_cpu.iter().copied().sum()))
            .collect();
        Self {
            at: Instant::now(),
            by_pid,
        }
    }

    /// Per-pid CPU% from the delta against `prev`, sorted and capped.
    fn utilisation_since(&self, prev: &Self, ncpus: f64) -> Vec<ProcessMetrics> {
        let elapsed_ns = self.at.duration_since(prev.at).as_nanos() as f64;

        let mut out: Vec<ProcessMetrics> = self
            .by_pid
            .iter()
            .map(|(&pid, &total)| {
                // New pids have no baseline yet → report 0% for one tick.
                let cpu_percent = match prev.by_pid.get(&pid) {
                    Some(&was) if elapsed_ns > 0.0 => {
                        let delta = total.saturating_sub(was) as f64;
                        (delta / elapsed_ns / ncpus * 100.0).clamp(0.0, 100.0)
                    }
                    _ => 0.0,
                };
                ProcessMetrics {
                    pid,
                    cpu: CpuMetrics { cpu_percent },
                    mem: None,
                    io: None,
                }
            })
            .collect();

        out.sort_by(|a, b| b.cpu.cpu_percent.total_cmp(&a.cpu.cpu_percent));
        out.truncate(MAX_ROWS);
        out
    }
}
