//! Collector (backend) — producer side of the double-buffer (CLAUDE.md §3).
//! Pulls the eBPF per-CPU counters at 1 Hz, derives per-interval CPU%, resolves
//! names, and publishes an immutable snapshot via an atomic pointer swap.
//!
//! Names use the event-driven identity model: the kernel captures `comm` on
//! `exec` into `COMM_MAP`; the local cache is seeded once from `/proc` and
//! filled from `COMM_MAP` on a miss, never re-parsing `/proc`.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use arc_swap::ArcSwap;
use aya::maps::{HashMap as BpfHashMap, MapData, PerCpuHashMap};
use tokio::time::{MissedTickBehavior, interval};
use truetop_common::COMM_LEN;

use crate::metrics::{CpuMetrics, ProcessMetrics};

const MAX_ROWS: usize = 256;
const UNKNOWN: &str = "<unknown>";

#[derive(Debug, Clone, Default)]
pub struct SystemState {
    pub tick: u64,
    pub processes: Vec<ProcessMetrics>,
}

/// Owns the persistent collection state; each [`Collector::tick`] reads the
/// counters and produces the next snapshot.
pub struct Collector {
    cpu_ns: PerCpuHashMap<MapData, u32, u64>,
    comm: BpfHashMap<MapData, u32, [u8; COMM_LEN]>,
    ncpus: f64,
    prev: Totals,
    names: HashMap<u32, String>,
    tick: u64,
}

impl Collector {
    pub fn new(
        cpu_ns: PerCpuHashMap<MapData, u32, u64>,
        comm: BpfHashMap<MapData, u32, [u8; COMM_LEN]>,
        ncpus: usize,
    ) -> Self {
        Self {
            cpu_ns,
            comm,
            ncpus: ncpus.max(1) as f64,
            prev: Totals::default(),
            names: backfill_proc_names(),
            tick: 0,
        }
    }

    fn tick(&mut self) -> SystemState {
        self.tick = self.tick.wrapping_add(1);

        let current = Totals::read(&self.cpu_ns);
        let mut processes = current.utilisation_since(&self.prev, self.ncpus);
        for p in &mut processes {
            p.name = self.name_for(p.pid);
        }
        self.prev = current;

        SystemState {
            tick: self.tick,
            processes,
        }
    }

    fn name_for(&mut self, pid: u32) -> String {
        if let Some(name) = self.names.get(&pid) {
            return name.clone();
        }
        // Cache successes only; a miss may resolve once a later `exec` lands.
        match self.comm.get(&pid, 0).ok().map(decode_comm) {
            Some(name) => {
                self.names.insert(pid, name.clone());
                name
            }
            None => UNKNOWN.to_owned(),
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

    fn utilisation_since(&self, prev: &Self, ncpus: f64) -> Vec<ProcessMetrics> {
        let elapsed_ns = self.at.duration_since(prev.at).as_nanos() as f64;

        let mut out: Vec<ProcessMetrics> = self
            .by_pid
            .iter()
            .map(|(&pid, &total)| {
                // No baseline on first sight → 0% for one tick.
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
                    ..Default::default()
                }
            })
            .collect();

        out.sort_by(|a, b| b.cpu.cpu_percent.total_cmp(&a.cpu.cpu_percent));
        out.truncate(MAX_ROWS);
        out
    }
}

fn decode_comm(raw: [u8; COMM_LEN]) -> String {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).trim().to_owned()
}

/// Seed names for processes that predate truetop and never fired a capturable
/// `exec`.
fn backfill_proc_names() -> HashMap<u32, String> {
    let mut names = HashMap::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return names;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse().ok()) else {
            continue;
        };
        if let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm")) {
            names.insert(pid, comm.trim().to_owned());
        }
    }
    names
}
