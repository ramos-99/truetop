//! CPU%: diffs the per-process on-CPU nanosecond counter (`CPU_NS`, read in
//! batch) between ticks. Returns a per-pid map; the backend selects and
//! enriches, the renderer sorts. The system total is normalised by core count
//! in the backend.

use std::collections::HashMap;

use super::Snapshot;

/// CPU share over the last interval, in top's convention: 100.0 is one core, so
/// a process running K threads flat out reads about K*100.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CpuMetrics {
    pub cpu_percent: f64,
}

pub(crate) struct CpuCollector {
    prev: Snapshot,
}

impl CpuCollector {
    pub(crate) fn new() -> Self {
        Self {
            prev: Snapshot::default(),
        }
    }

    /// Per-process CPU% for the interval, keyed by pid. First-sight pids read 0%
    /// (no baseline); multithreaded processes can exceed 100%.
    ///
    /// A counter that goes backwards reads 0% for one interval and then reports
    /// normally. Two things do that: an entry evicted from a full LRU map and
    /// re-created when its process next runs, and a recycled pid inheriting the
    /// larger total of the dead process that held it. Both resolve themselves on
    /// the following tick.
    pub(crate) fn deltas(&mut self, current: Snapshot) -> HashMap<u32, f64> {
        let elapsed_ns = current.elapsed_ns_since(&self.prev);
        let out = current
            .by_pid
            .iter()
            .map(|(&pid, &total)| {
                let percent = match self.prev.by_pid.get(&pid) {
                    Some(&was) if elapsed_ns > 0.0 => {
                        total.saturating_sub(was) as f64 / elapsed_ns * 100.0
                    }
                    _ => 0.0,
                };
                (pid, percent)
            })
            .collect();
        self.prev = current;
        out
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn collector(prev: &[(u32, u64)], base: Instant) -> CpuCollector {
        let mut c = CpuCollector::new();
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
    fn zero_without_baseline() {
        let base = Instant::now();
        let mut c = CpuCollector::new();
        let out = c.deltas(snapshot(
            base + Duration::from_secs(1),
            &[(1, 1_000_000_000)],
        ));
        assert_eq!(out[&1], 0.0);
    }

    // One core-second of on-CPU time over a one-second interval is 100%.
    #[test]
    fn full_core_reads_100() {
        let base = Instant::now();
        let mut c = collector(&[(1, 0)], base);
        let out = c.deltas(snapshot(
            base + Duration::from_secs(1),
            &[(1, 1_000_000_000)],
        ));
        assert!((out[&1] - 100.0).abs() < 1e-6);
    }

    // Top's convention: eight thread-seconds in one second read ~800%, unclamped.
    #[test]
    fn multithread_exceeds_100() {
        let base = Instant::now();
        let mut c = collector(&[(1, 0)], base);
        let out = c.deltas(snapshot(
            base + Duration::from_secs(1),
            &[(1, 8_000_000_000)],
        ));
        assert!((out[&1] - 800.0).abs() < 1e-6);
    }

    /// Eviction and pid reuse both land here: the counter is smaller than the
    /// baseline, so the interval reads zero rather than a negative rate.
    #[test]
    fn saturates_on_counter_reset() {
        let base = Instant::now();
        let mut c = collector(&[(1, 1_000_000_000)], base);
        let out = c.deltas(snapshot(base + Duration::from_secs(1), &[(1, 0)]));
        assert_eq!(out[&1], 0.0);
    }
}
