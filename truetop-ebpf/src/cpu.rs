//! CPU utilisation (CLAUDE.md §2). Per-CPU, add-only counters driven from the
//! shared `sched_switch` hook (`sched`); user space derives percentages from
//! cross-tick deltas.
//!
//! Time is charged per thread but accumulated per process. A CPU runs exactly
//! one thread at a time and a thread cannot migrate while on-CPU, so the
//! schedule-in stopwatch is a single per-CPU slot (`START_TIME`), not a
//! tid-keyed map: `mark_in` stamps it, and the next `charge_out` on the same CPU
//! reads it back and bills the slice to the process total (`CPU_NS`, keyed by
//! tgid), matching top/btop. Idle (tid 0) is neither stamped nor charged, so a
//! real `prev` always finds its own schedule-in time in the slot.

use aya_ebpf::{
    macros::map,
    maps::{PerCpuArray, PerCpuHashMap},
};

// Schedule-in timestamp of the thread currently on this CPU. One running thread
// per CPU means one slot; 0 means nothing is stamped yet (the first switch seen
// on this CPU), so there is no baseline to charge from.
#[map]
static START_TIME: PerCpuArray<u64> = PerCpuArray::with_max_entries(1, 0);
// tgid → accumulated on-CPU nanoseconds (the counter user space diffs).
#[map]
static CPU_NS: PerCpuHashMap<u32, u64> = PerCpuHashMap::with_max_entries(16384, 0);

/// Add the slice the outgoing thread just ran to its process total.
#[inline(always)]
pub(crate) fn charge_out(tid: u32, tgid: u32, now: u64) {
    if tid == 0 {
        return;
    }
    let start = START_TIME.get(0).copied().unwrap_or(0);
    if start == 0 {
        return;
    }
    let slice = now.saturating_sub(start);
    match CPU_NS.get_ptr_mut(tgid) {
        // In-place add on the returned pointer: one map op, not get + insert.
        Some(total) => unsafe { *total = (*total).saturating_add(slice) },
        None => {
            let _ = CPU_NS.insert(tgid, slice, 0);
        }
    }
}

/// Stamp the incoming thread's schedule-in time in this CPU's slot.
#[inline(always)]
pub(crate) fn mark_in(tid: u32, now: u64) {
    if tid == 0 {
        return;
    }
    let _ = START_TIME.set(0, now, 0);
}
