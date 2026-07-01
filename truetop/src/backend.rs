//! Collector (backend) - producer side of the double-buffer (CLAUDE.md §3).
//! Pulls the eBPF CPU counters at 1 Hz via one batched map read, derives
//! per-interval CPU%, resolves names and RSS, and publishes an immutable
//! snapshot via an atomic pointer swap.
//!
//! Names use the event-driven identity model: the kernel captures `comm` on
//! `exec` into `COMM_MAP`; a one-time `/proc` snapshot seeds processes that
//! predate truetop. RSS is read from `/proc/<pid>/statm` for the visible rows
//! only - since Linux 6.2 the exact value cannot be summed from eBPF cheaply
//! (see README), so we read the same source `top` does, at negligible cost.

use std::{
    collections::HashMap,
    os::fd::{AsFd, AsRawFd},
    sync::Arc,
    time::{Duration, Instant},
};

use arc_swap::ArcSwap;
use aya::maps::{HashMap as BpfHashMap, MapData};
use tokio::time::{MissedTickBehavior, interval};
use truetop_common::COMM_LEN;

use crate::{
    batch::BatchReader,
    metrics::{CpuMetrics, MemMetrics, ProcessMetrics},
};

const MAX_ROWS: usize = 256;
const UNKNOWN: &str = "<unknown>";

#[derive(Debug, Clone, Default)]
pub struct SystemState {
    pub processes: Vec<ProcessMetrics>,
}

/// Owns the persistent collection state; each [`Collector::tick`] reads the maps
/// and produces the next snapshot.
pub struct Collector {
    cpu_ns: MapData,
    reader: BatchReader,
    comm: BpfHashMap<MapData, u32, [u8; COMM_LEN]>,
    ncpus: f64,
    page_size: u64,
    prev: Totals,
    name_seed: HashMap<u32, String>,
}

impl Collector {
    pub fn new(
        cpu_ns: MapData,
        comm: BpfHashMap<MapData, u32, [u8; COMM_LEN]>,
        ncpus: usize,
    ) -> Self {
        let ncpus = ncpus.max(1);
        Self {
            cpu_ns,
            reader: BatchReader::new(ncpus),
            comm,
            ncpus: ncpus as f64,
            page_size: page_size(),
            prev: Totals::default(),
            name_seed: backfill_proc_names(),
        }
    }

    pub fn tick(&mut self) -> SystemState {
        let fd = self.cpu_ns.fd().as_fd().as_raw_fd();
        let current = Totals {
            at: Some(Instant::now()),
            by_pid: self.reader.sum_per_cpu(fd),
        };

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

    /// Exact RSS from `/proc/<tgid>/statm`. `None` if the process exited between
    /// the snapshot and this read.
    fn rss_for(&self, tgid: u32) -> Option<MemMetrics> {
        let statm = std::fs::read_to_string(format!("/proc/{tgid}/statm")).ok()?;
        Some(MemMetrics {
            rss_bytes: parse_statm_pages(&statm)? * self.page_size,
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

/// Run `ticks` collector iterations at 1 Hz with no UI, for tracing the
/// collection path without the render loop's syscalls.
pub fn run_headless(mut collector: Collector, ticks: u32) {
    for i in 1..=ticks {
        std::thread::sleep(Duration::from_secs(1));
        let snapshot = collector.tick();
        println!("tick {i}/{ticks}: {} processes", snapshot.processes.len());
    }
}

/// Cumulative on-CPU nanoseconds per process at one instant, plus when it was
/// taken.
#[derive(Default)]
struct Totals {
    at: Option<Instant>,
    by_pid: HashMap<u32, u64>,
}

impl Totals {
    fn utilisation_since(&self, prev: &Self, ncpus: f64) -> Vec<ProcessMetrics> {
        let elapsed_ns = match (self.at, prev.at) {
            (Some(now), Some(was)) => now.duration_since(was).as_nanos() as f64,
            _ => 0.0,
        };

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

/// Resident-set pages from a `/proc/<pid>/statm` line - the second whitespace
/// field. `None` if the line is empty or malformed.
fn parse_statm_pages(statm: &str) -> Option<u64> {
    statm.split_whitespace().nth(1)?.parse().ok()
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn comm(bytes: &[u8]) -> [u8; COMM_LEN] {
        let mut raw = [0u8; COMM_LEN];
        raw[..bytes.len()].copy_from_slice(bytes);
        raw
    }

    fn totals(at: Instant, pairs: &[(u32, u64)]) -> Totals {
        Totals {
            at: Some(at),
            by_pid: pairs.iter().copied().collect(),
        }
    }

    #[test]
    fn parse_statm_reads_resident_pages() {
        assert_eq!(parse_statm_pages("4096 512 64 1 0 128 0"), Some(512));
        assert_eq!(parse_statm_pages("  4096   512  "), Some(512));
    }

    #[test]
    fn parse_statm_rejects_malformed() {
        assert_eq!(parse_statm_pages(""), None);
        assert_eq!(parse_statm_pages("4096"), None);
        assert_eq!(parse_statm_pages("4096 notanumber"), None);
    }

    #[test]
    fn decode_comm_stops_at_nul() {
        assert_eq!(decode_comm(comm(b"bash\0ignored")), "bash");
    }

    #[test]
    fn decode_comm_reads_full_buffer() {
        assert_eq!(decode_comm([b'a'; COMM_LEN]), "a".repeat(COMM_LEN));
    }

    #[test]
    fn decode_comm_is_lossy_on_invalid_utf8() {
        assert_eq!(decode_comm(comm(&[0xff, 0xff])), "\u{fffd}\u{fffd}");
    }

    #[test]
    fn utilisation_is_zero_without_baseline() {
        let base = Instant::now();
        let prev = totals(base, &[]);
        let cur = totals(base + Duration::from_secs(1), &[(1, 1_000_000_000)]);
        let rows = cur.utilisation_since(&prev, 1.0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].cpu.cpu_percent, 0.0);
    }

    #[test]
    fn utilisation_scales_by_cpu_count() {
        let base = Instant::now();
        let prev = totals(base, &[(1, 0)]);
        // One core-second of CPU over a one-second interval.
        let cur = totals(base + Duration::from_secs(1), &[(1, 1_000_000_000)]);
        assert!((cur.utilisation_since(&prev, 1.0)[0].cpu.cpu_percent - 100.0).abs() < 1e-6);
        assert!((cur.utilisation_since(&prev, 4.0)[0].cpu.cpu_percent - 25.0).abs() < 1e-6);
    }

    #[test]
    fn utilisation_saturates_on_counter_reset() {
        let base = Instant::now();
        let prev = totals(base, &[(1, 1_000_000_000)]);
        let cur = totals(base + Duration::from_secs(1), &[(1, 0)]); // PID reuse
        assert_eq!(cur.utilisation_since(&prev, 1.0)[0].cpu.cpu_percent, 0.0);
    }

    #[test]
    fn utilisation_clamps_to_single_core() {
        let base = Instant::now();
        let prev = totals(base, &[(1, 0)]);
        // Ten core-seconds in one second is 1000%; clamp to 100.
        let cur = totals(base + Duration::from_secs(1), &[(1, 10_000_000_000)]);
        assert_eq!(cur.utilisation_since(&prev, 1.0)[0].cpu.cpu_percent, 100.0);
    }

    #[test]
    fn utilisation_is_sorted_and_capped() {
        let base = Instant::now();
        let count = MAX_ROWS as u32 + 50;
        // Zero baseline so each PID's delta is its id: busier PIDs have higher ids.
        let prev = totals(base, &(1..=count).map(|p| (p, 0)).collect::<Vec<_>>());
        let cur = totals(
            base + Duration::from_secs(1),
            &(1..=count).map(|p| (p, p as u64)).collect::<Vec<_>>(),
        );
        let rows = cur.utilisation_since(&prev, 1.0);
        assert_eq!(rows.len(), MAX_ROWS);
        assert!(
            rows.windows(2)
                .all(|w| w[0].cpu.cpu_percent >= w[1].cpu.cpu_percent)
        );
        assert_eq!(rows[0].pid, count);
    }
}
