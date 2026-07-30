//! CPU utilisation. Per-CPU, add-only counters driven from the shared
//! `sched_switch` hook (`sched`); user space derives percentages from cross-tick
//! deltas.
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
    maps::{LruPerCpuHashMap, PerCpuArray},
};

// Schedule-in timestamp of the thread currently on this CPU. One running thread
// per CPU means one slot; 0 means nothing is stamped yet (the first switch seen
// on this CPU), so there is no baseline to charge from.
#[map]
static START_TIME: PerCpuArray<u64> = PerCpuArray::with_max_entries(1, 0);
// tgid → accumulated on-CPU nanoseconds (the counter user space diffs). LRU: a
// full map evicts its coldest entry rather than refusing the write, and the
// coldest entry is what a departed process leaves behind, so eviction is also
// how entries are reclaimed. The per-tick read does not disturb that order - the
// kernel does not mark entries on the syscall lookup path.
#[map]
static CPU_NS: LruPerCpuHashMap<u32, u64> = LruPerCpuHashMap::with_max_entries(16384, 0);
// tgid of the thread currently on this CPU (0 = idle), paired with START_TIME so
// user space can bill a running thread's in-flight slice at read (`add_inflight`).
#[map]
static CURRENT_TGID: PerCpuArray<u32> = PerCpuArray::with_max_entries(1, 0);

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

/// Record the incoming thread as the one now on this CPU.
#[inline(always)]
pub(crate) fn mark_in(tid: u32, tgid: u32, now: u64) {
    // START_TIME first: a user-space read landing between these two writes then
    // pairs the outgoing tgid with a fresh timestamp - a sliver of nanoseconds
    // misattributed - instead of the incoming tgid with a stale one, which on a
    // CPU waking from a long idle could bill it the whole idle gap.
    if tid != 0 {
        let _ = START_TIME.set(0, now, 0);
    }
    let _ = CURRENT_TGID.set(0, tgid, 0);
}
