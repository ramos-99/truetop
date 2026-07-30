//! I/O wait: time in uninterruptible (D-state) sleep, charged to the task that
//! actually blocks. A synchronous read charges the process that issued it;
//! deferred writeback charges the flush kworker, since that is the task the
//! kernel puts to sleep. Stamp on D switch-out, charge tgid on the next
//! switch-in.
//!
//! `SLEEP_SINCE` is global, not per-CPU: a D interval spans CPUs, so per-CPU
//! storage cannot pair its edges. Lookups are lock-free RCU; locked updates
//! happen only on D transitions.

use aya_ebpf::{
    macros::map,
    maps::{HashMap, LruPerCpuHashMap},
};

// include/linux/sched.h; NOLOAD excluded to skip TASK_IDLE kthreads.
const TASK_UNINTERRUPTIBLE: u32 = 0x2;
const TASK_NOLOAD: u32 = 0x400;

// tid → D-state switch-out timestamp. Plain, not LRU: an entry lives only from a
// D switch-out to the next switch-in, so this is bounded by the threads asleep
// at one instant rather than by the process count. The residual limit is real
// though: past 16,384 concurrent D-sleepers a stamp is dropped and that thread's
// interval goes unmeasured.
#[map]
static SLEEP_SINCE: HashMap<u32, u64> = HashMap::with_max_entries(16384, 0);
// tgid → accumulated D-sleep nanoseconds (the counter user space diffs). LRU for
// the same reason as `CPU_NS`.
#[map]
static IOWAIT_NS: LruPerCpuHashMap<u32, u64> = LruPerCpuHashMap::with_max_entries(16384, 0);

#[inline(always)]
pub(crate) fn sleep_out(state: u32, tid: u32, now: u64) {
    if tid == 0 {
        return;
    }
    if state & (TASK_UNINTERRUPTIBLE | TASK_NOLOAD) == TASK_UNINTERRUPTIBLE {
        let _ = SLEEP_SINCE.insert(tid, now, 0);
    }
}

#[inline(always)]
pub(crate) fn wake_in(tgid: u32, tid: u32, now: u64) {
    if tid == 0 {
        return;
    }
    let Some(since) = (unsafe { SLEEP_SINCE.get(tid) }).copied() else {
        return;
    };
    let _ = SLEEP_SINCE.remove(tid);
    if tgid == 0 {
        return;
    }
    let slept = now.saturating_sub(since);
    match IOWAIT_NS.get_ptr_mut(tgid) {
        // In-place add on the returned pointer: one map op, not get + insert,
        // which on an LRU map would reallocate the entry's node.
        Some(total) => unsafe { *total = (*total).saturating_add(slept) },
        None => {
            let _ = IOWAIT_NS.insert(tgid, slept, 0);
        }
    }
}

/// Drop a departed thread's sleep stamp. The `IOWAIT_NS` accumulator is not
/// touched here: it is per-process accounting that user space reaps once it has
/// read the final total (see `exit_notify`).
#[inline(always)]
pub(crate) fn forget_thread(tid: u32) {
    let _ = SLEEP_SINCE.remove(tid);
}
