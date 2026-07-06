//! CPU%: the driver metric. Diffs the per-process on-CPU nanosecond counter
//! (`CPU_NS`, read in batch) between ticks and normalises to system capacity,
//! then establishes the sorted, capped row set the other metrics enrich.

use super::{ProcessMetrics, Snapshot};

/// CPU share over the last interval, normalised to whole-system capacity
/// (100.0 = every logical core saturated).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CpuMetrics {
    pub cpu_percent: f64,
}

/// Viewport cap: only this many rows are kept and enriched.
const MAX_ROWS: usize = 256;

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

    /// Derive per-process CPU% from the counter delta, sorted desc and capped to
    /// the viewport. The driver: it establishes the row set.
    pub(crate) fn collect(&mut self, current: Snapshot) -> Vec<ProcessMetrics> {
        let rows = utilisation(&current, &self.prev, self.ncpus);
        self.prev = current;
        rows
    }
}

fn utilisation(current: &Snapshot, prev: &Snapshot, ncpus: f64) -> Vec<ProcessMetrics> {
    let elapsed_ns = current.elapsed_ns_since(prev);
    let mut out: Vec<ProcessMetrics> = current
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

    #[test]
    fn utilisation_is_zero_without_baseline() {
        let base = Instant::now();
        let prev = snapshot(base, &[]);
        let cur = snapshot(base + Duration::from_secs(1), &[(1, 1_000_000_000)]);
        let rows = utilisation(&cur, &prev, 1.0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].cpu.cpu_percent, 0.0);
    }

    #[test]
    fn utilisation_scales_by_cpu_count() {
        let base = Instant::now();
        let prev = snapshot(base, &[(1, 0)]);
        let cur = snapshot(base + Duration::from_secs(1), &[(1, 1_000_000_000)]);
        assert!((utilisation(&cur, &prev, 1.0)[0].cpu.cpu_percent - 100.0).abs() < 1e-6);
        assert!((utilisation(&cur, &prev, 4.0)[0].cpu.cpu_percent - 25.0).abs() < 1e-6);
    }

    #[test]
    fn utilisation_saturates_on_counter_reset() {
        let base = Instant::now();
        let prev = snapshot(base, &[(1, 1_000_000_000)]);
        let cur = snapshot(base + Duration::from_secs(1), &[(1, 0)]);
        assert_eq!(utilisation(&cur, &prev, 1.0)[0].cpu.cpu_percent, 0.0);
    }

    // Ten core-seconds in one second would be 1000%; the result clamps to 100.
    #[test]
    fn utilisation_clamps_to_single_core() {
        let base = Instant::now();
        let prev = snapshot(base, &[(1, 0)]);
        let cur = snapshot(base + Duration::from_secs(1), &[(1, 10_000_000_000)]);
        assert_eq!(utilisation(&cur, &prev, 1.0)[0].cpu.cpu_percent, 100.0);
    }

    // Zero baseline makes each PID's delta equal its id, so busier PIDs sort first.
    #[test]
    fn utilisation_is_sorted_and_capped() {
        let base = Instant::now();
        let count = MAX_ROWS as u32 + 50;
        let prev = snapshot(base, &(1..=count).map(|p| (p, 0)).collect::<Vec<_>>());
        let cur = snapshot(
            base + Duration::from_secs(1),
            &(1..=count).map(|p| (p, p as u64)).collect::<Vec<_>>(),
        );
        let rows = utilisation(&cur, &prev, 1.0);
        assert_eq!(rows.len(), MAX_ROWS);
        assert!(
            rows.windows(2)
                .all(|w| w[0].cpu.cpu_percent >= w[1].cpu.cpu_percent)
        );
        assert_eq!(rows[0].pid, count);
    }
}
