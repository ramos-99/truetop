//! I/O wait per process: share of the interval spent blocked in
//! uninterruptible (D-state) sleep, i.e. waiting on the disk. Mirrors `cpu` -
//! a per-tgid nanosecond counter (`IOWAIT_NS`) diffed between ticks.

use super::{ProcessMetrics, Snapshot};

/// Share of the last interval spent in uninterruptible I/O sleep. Summed over
/// threads, so it can exceed 100 when several block concurrently.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct IoMetrics {
    pub io_wait_percent: f64,
}

pub(crate) struct IoWaitCollector {
    prev: Snapshot,
}

impl IoWaitCollector {
    pub(crate) fn new() -> Self {
        Self {
            prev: Snapshot::default(),
        }
    }

    /// Enrich the visible rows with the interval's I/O wait share. Rows without
    /// a baseline (first sight) or without any D-sleep history stay `None`.
    pub(crate) fn enrich(&mut self, current: Snapshot, rows: &mut [ProcessMetrics]) {
        let elapsed_ns = current.elapsed_ns_since(&self.prev);
        if elapsed_ns > 0.0 {
            for row in rows.iter_mut() {
                let (Some(&now), Some(&was)) =
                    (current.by_pid.get(&row.pid), self.prev.by_pid.get(&row.pid))
                else {
                    continue;
                };
                row.io = Some(IoMetrics {
                    io_wait_percent: now.saturating_sub(was) as f64 / elapsed_ns * 100.0,
                });
            }
        }
        self.prev = current;
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn snapshot(at: Instant, pairs: &[(u32, u64)]) -> Snapshot {
        Snapshot {
            at: Some(at),
            by_pid: pairs.iter().copied().collect(),
        }
    }

    fn rows(pids: &[u32]) -> Vec<ProcessMetrics> {
        pids.iter()
            .map(|&pid| ProcessMetrics {
                pid,
                ..Default::default()
            })
            .collect()
    }

    fn enrich_once(prev: Snapshot, current: Snapshot, rows: &mut [ProcessMetrics]) {
        let mut collector = IoWaitCollector::new();
        collector.prev = prev;
        collector.enrich(current, rows);
    }

    #[test]
    fn percent_from_counter_delta() {
        let base = Instant::now();
        let mut rows = rows(&[1]);
        enrich_once(
            snapshot(base, &[(1, 0)]),
            snapshot(base + Duration::from_secs(1), &[(1, 100_000_000)]),
            &mut rows,
        );
        assert!((rows[0].io.unwrap().io_wait_percent - 10.0).abs() < 1e-6);
    }

    // Two threads blocked for the whole interval sum to 200%.
    #[test]
    fn concurrent_thread_waits_exceed_100() {
        let base = Instant::now();
        let mut rows = rows(&[1]);
        enrich_once(
            snapshot(base, &[(1, 0)]),
            snapshot(base + Duration::from_secs(1), &[(1, 2_000_000_000)]),
            &mut rows,
        );
        assert!((rows[0].io.unwrap().io_wait_percent - 200.0).abs() < 1e-6);
    }

    #[test]
    fn none_without_baseline() {
        let base = Instant::now();
        let mut rows = rows(&[1]);
        enrich_once(
            snapshot(base, &[]),
            snapshot(base + Duration::from_secs(1), &[(1, 1_000_000)]),
            &mut rows,
        );
        assert_eq!(rows[0].io, None);
    }

    #[test]
    fn none_without_any_wait_history() {
        let base = Instant::now();
        let mut rows = rows(&[7]);
        enrich_once(
            snapshot(base, &[(1, 0)]),
            snapshot(base + Duration::from_secs(1), &[(1, 5)]),
            &mut rows,
        );
        assert_eq!(rows[0].io, None);
    }

    #[test]
    fn saturates_on_counter_reset() {
        let base = Instant::now();
        let mut rows = rows(&[1]);
        enrich_once(
            snapshot(base, &[(1, 1_000_000_000)]),
            snapshot(base + Duration::from_secs(1), &[(1, 0)]),
            &mut rows,
        );
        assert_eq!(rows[0].io.unwrap().io_wait_percent, 0.0);
    }
}
