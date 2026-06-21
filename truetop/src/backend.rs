//! Collector (backend) — producer side of the double-buffer (CLAUDE.md §3).
//! Pulls the eBPF CPU counters at 1 Hz, derives per-interval CPU%, resolves
//! names and RSS, and publishes an immutable snapshot via an atomic pointer swap.
//!
//! Names use the event-driven identity model: the kernel captures `comm` on
//! `exec` into `COMM_MAP`; a one-time `/proc` snapshot seeds processes that
//! predate truetop. RSS is read from `/proc/<pid>/statm` for the visible rows
//! only — since Linux 6.2 the exact value cannot be summed from eBPF cheaply
//! (see README), so we read the same source `top` does, at negligible cost.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use arc_swap::ArcSwap;
use aya::maps::{HashMap as BpfHashMap, MapData, PerCpuHashMap};
use tokio::time::{MissedTickBehavior, interval};
use truetop_common::COMM_LEN;

use crate::metrics::{CpuMetrics, MemMetrics, ProcessMetrics};

const MAX_ROWS: usize = 256;
const UNKNOWN: &str = "<unknown>";

#[derive(Debug, Clone, Default)]
pub struct SystemState {
    pub processes: Vec<ProcessMetrics>,
}

/// Owns the persistent collection state; each [`Collector::tick`] reads the maps
/// and produces the next snapshot.
pub struct Collector {
    cpu_ns: PerCpuHashMap<MapData, u32, u64>,
    comm: BpfHashMap<MapData, u32, [u8; COMM_LEN]>,
    ncpus: f64,
    page_size: u64,
    prev: Totals,
    name_seed: HashMap<u32, String>,
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
            page_size: page_size(),
            prev: Totals::default(),
            name_seed: backfill_proc_names(),
        }
    }

    fn tick(&mut self) -> SystemState {
        let current = Totals::read(&self.cpu_ns);
        // Sorted and capped to the viewport; only these rows are enriched.
        let mut processes = current.utilisation_since(&self.prev, self.ncpus);
        for p in &mut processes {
            p.name = self.name_for(p.pid);
            p.mem = self.rss_for(p.pid);
        }
        self.prev = current;

        SystemState { processes }
    }

    /// Live `COMM_MAP` wins; fall back to the startup `/proc` snapshot.
    fn name_for(&self, tgid: u32) -> String {
        if let Ok(comm) = self.comm.get(&tgid, 0) {
            return decode_comm(comm);
        }
        self.name_seed
            .get(&tgid)
            .cloned()
            .unwrap_or_else(|| UNKNOWN.to_owned())
    }

    /// Exact RSS from `/proc/<tgid>/statm` (field 1, resident pages). `None` if
    /// the process exited between the snapshot and this read.
    fn rss_for(&self, tgid: u32) -> Option<MemMetrics> {
        let statm = std::fs::read_to_string(format!("/proc/{tgid}/statm")).ok()?;
        let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
        Some(MemMetrics {
            rss_bytes: pages * self.page_size,
        })
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

/// Cumulative on-CPU nanoseconds per process at one instant, plus when it was
/// taken.
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

fn page_size() -> u64 {
    // SAFETY: sysconf with a valid name is always safe to call.
    let n = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if n > 0 { n as u64 } else { 4096 }
}

/// Seed names for processes that predate truetop and never fired a capturable
/// `exec`.
fn backfill_proc_names() -> HashMap<u32, String> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return HashMap::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let pid = e.file_name().to_str()?.parse().ok()?;
            let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
            Some((pid, comm.trim().to_owned()))
        })
        .collect()
}
