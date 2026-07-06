//! I/O wait per process — time blocked in uninterruptible (D-state) sleep, i.e.
//! waiting on the disk. Phase 2: mirrors `cpu` (a counter delta over the
//! interval) but is not yet wired to a kernel counter.

use super::{ProcessMetrics, Snapshot};

/// Nanoseconds a process spent blocked in uninterruptible I/O sleep over the
/// last interval.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
#[allow(dead_code)]
pub struct IoMetrics {
    pub io_wait_ns: u64,
}

#[allow(dead_code)]
pub(crate) struct IoWaitCollector {
    prev: Snapshot,
}

#[allow(dead_code)]
impl IoWaitCollector {
    pub(crate) fn new() -> Self {
        Self {
            prev: Snapshot::default(),
        }
    }

    /// Enrich the visible rows with per-interval I/O wait, mirroring `cpu`'s
    /// counter delta.
    pub(crate) fn enrich(&mut self, current: Snapshot, rows: &mut [ProcessMetrics]) {
        for row in rows.iter_mut() {
            if let (Some(&now), Some(&was)) =
                (current.by_pid.get(&row.pid), self.prev.by_pid.get(&row.pid))
            {
                row.io = Some(IoMetrics {
                    io_wait_ns: now.saturating_sub(was),
                });
            }
        }
        self.prev = current;
    }
}
