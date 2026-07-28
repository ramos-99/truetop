//! Closing the books on departed processes.
//!
//! The exit tracepoint leaves each process's final CPU and I/O-wait totals in
//! the counter maps and announces the departure on a ring buffer (see the eBPF
//! `exit_notify`). A collector tick reads those totals like any other, so a
//! process that lived and died between two ticks is still charged its full time.
//! Only after that read does the reaper delete the entries, here.
//!
//! Draining the ring costs no syscall - it is shared memory. Deleting is one map
//! operation per departed process, on a path that runs at the exit rate, far
//! below the context-switch rate the hotpath answers to.

use std::os::fd::{AsFd, AsRawFd};

use aya::maps::{MapData, RingBuf};
use truetop_common::ExitEvent;

use crate::{batch::delete_elem, metrics::Resolver};

pub(crate) struct Reaper {
    exits: RingBuf<MapData>,
}

impl Reaper {
    pub(crate) fn new(exits: RingBuf<MapData>) -> Self {
        Self { exits }
    }

    /// Departed processes cleared per tick. Three map operations each, so an
    /// exit storm would otherwise queue tens of thousands of syscalls ahead of
    /// the snapshot. Keeps up with the heaviest churn we benchmark, ~3,300
    /// exits/s.
    const BUDGET: usize = 4096;

    /// Delete announced processes from the accounting maps and the name cache,
    /// up to [`Self::BUDGET`]. Run after the tick has read this interval's
    /// totals, so a process that just ended is charged its final slice before
    /// its entry goes.
    ///
    /// Records past the budget wait in the ring for the next tick, and any the
    /// ring drops leave a cold entry for eviction to reclaim (see `cpu`).
    pub(crate) fn reap(
        &mut self,
        cpu_ns: &mut MapData,
        iowait_ns: &mut MapData,
        names: &mut Resolver,
    ) {
        for _ in 0..Self::BUDGET {
            let Some(record) = self.exits.next() else {
                break;
            };
            if let Some(event) = ExitEvent::from_bytes(&record) {
                delete_elem(fd(cpu_ns), event.tgid);
                delete_elem(fd(iowait_ns), event.tgid);
                names.forget(event.tgid);
            }
        }
    }
}

fn fd(map: &MapData) -> std::os::fd::RawFd {
    map.fd().as_fd().as_raw_fd()
}
