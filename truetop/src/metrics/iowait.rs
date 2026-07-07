//! I/O wait per process: share of the interval spent blocked in uninterruptible
//! (D-state) sleep. Mirrors `cpu` - diffs the per-tgid nanosecond counter
//! (`IOWAIT_NS`) between ticks into a per-pid map.

use std::collections::HashMap;

use super::Snapshot;

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

    /// Per-process I/O-wait share for the interval, keyed by pid. Only pids with
    /// a baseline appear (first-sight pids wait until the next tick).
    pub(crate) fn deltas(&mut self, current: Snapshot) -> HashMap<u32, f64> {
        let elapsed_ns = current.elapsed_ns_since(&self.prev);
        let out = if elapsed_ns > 0.0 {
            current
                .by_pid
                .iter()
                .filter_map(|(&pid, &now)| {
                    let was = *self.prev.by_pid.get(&pid)?;
                    Some((pid, now.saturating_sub(was) as f64 / elapsed_ns * 100.0))
                })
                .collect()
        } else {
            HashMap::new()
        };
        self.prev = current;
        out
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn collector(prev: &[(u32, u64)], base: Instant) -> IoWaitCollector {
        let mut c = IoWaitCollector::new();
        c.prev = Snapshot {
            at: Some(base),
            by_pid: prev.iter().copied().collect(),
        };
        c
    }

    fn snapshot(at: Instant, pairs: &[(u32, u64)]) -> Snapshot {
        Snapshot {
            at: Some(at),
            by_pid: pairs.iter().copied().collect(),
        }
    }

    #[test]
    fn percent_from_counter_delta() {
        let base = Instant::now();
        let mut c = collector(&[(1, 0)], base);
        let out = c.deltas(snapshot(base + Duration::from_secs(1), &[(1, 100_000_000)]));
        assert!((out[&1] - 10.0).abs() < 1e-6);
    }

    // Two threads blocked for the whole interval sum to 200%.
    #[test]
    fn concurrent_thread_waits_exceed_100() {
        let base = Instant::now();
        let mut c = collector(&[(1, 0)], base);
        let out = c.deltas(snapshot(
            base + Duration::from_secs(1),
            &[(1, 2_000_000_000)],
        ));
        assert!((out[&1] - 200.0).abs() < 1e-6);
    }

    #[test]
    fn absent_without_baseline() {
        let base = Instant::now();
        let mut c = IoWaitCollector::new();
        let out = c.deltas(snapshot(base + Duration::from_secs(1), &[(1, 1_000_000)]));
        assert!(!out.contains_key(&1));
    }

    #[test]
    fn saturates_on_counter_reset() {
        let base = Instant::now();
        let mut c = collector(&[(1, 1_000_000_000)], base);
        let out = c.deltas(snapshot(base + Duration::from_secs(1), &[(1, 0)]));
        assert_eq!(out[&1], 0.0);
    }
}
