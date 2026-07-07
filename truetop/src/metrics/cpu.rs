//! CPU%: diffs the per-process on-CPU nanosecond counter (`CPU_NS`, read in
//! batch) between ticks and normalises to system capacity. Returns a per-pid
//! map; the backend selects and enriches, the renderer sorts.

use std::collections::HashMap;

use super::Snapshot;

/// CPU share over the last interval, normalised to whole-system capacity
/// (100.0 = every logical core saturated).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CpuMetrics {
    pub cpu_percent: f64,
}

pub(crate) struct CpuCollector {
    prev: Snapshot,
    ncpus: f64,
}

impl CpuCollector {
    pub(crate) fn new(ncpus: f64) -> Self {
        Self {
            prev: Snapshot::default(),
            ncpus,
        }
    }

    /// Per-process CPU% for the interval, keyed by pid. First-sight pids read 0%
    /// (no baseline).
    pub(crate) fn deltas(&mut self, current: Snapshot) -> HashMap<u32, f64> {
        let elapsed_ns = current.elapsed_ns_since(&self.prev);
        let out = current
            .by_pid
            .iter()
            .map(|(&pid, &total)| {
                let percent = match self.prev.by_pid.get(&pid) {
                    Some(&was) if elapsed_ns > 0.0 => {
                        let delta = total.saturating_sub(was) as f64;
                        (delta / elapsed_ns / self.ncpus * 100.0).clamp(0.0, 100.0)
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

    fn collector(prev: &[(u32, u64)], base: Instant, ncpus: f64) -> CpuCollector {
        let mut c = CpuCollector::new(ncpus);
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
        let mut c = CpuCollector::new(1.0);
        let out = c.deltas(snapshot(
            base + Duration::from_secs(1),
            &[(1, 1_000_000_000)],
        ));
        assert_eq!(out[&1], 0.0);
    }

    #[test]
    fn scales_by_cpu_count() {
        let base = Instant::now();
        let cur = snapshot(base + Duration::from_secs(1), &[(1, 1_000_000_000)]);
        assert!((collector(&[(1, 0)], base, 1.0).deltas(cur.clone())[&1] - 100.0).abs() < 1e-6);
        assert!((collector(&[(1, 0)], base, 4.0).deltas(cur)[&1] - 25.0).abs() < 1e-6);
    }

    #[test]
    fn saturates_on_counter_reset() {
        let base = Instant::now();
        let mut c = collector(&[(1, 1_000_000_000)], base, 1.0);
        assert_eq!(
            c.deltas(snapshot(base + Duration::from_secs(1), &[(1, 0)]))[&1],
            0.0
        );
    }

    #[test]
    fn clamps_to_single_core() {
        let base = Instant::now();
        let mut c = collector(&[(1, 0)], base, 1.0);
        let out = c.deltas(snapshot(
            base + Duration::from_secs(1),
            &[(1, 10_000_000_000)],
        ));
        assert_eq!(out[&1], 100.0);
    }
}
