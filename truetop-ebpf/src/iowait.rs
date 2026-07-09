//! I/O wait: time in uninterruptible (D-state) sleep, charged to the task that
//! actually blocks. A synchronous read charges the process that issued it;
//! deferred writeback charges the flush kworker, since that is the task the
//! kernel puts to sleep. Stamp on D switch-out, charge tgid on the next
//! switch-in.
//!
//! `SLEEP_SINCE` is global, not per-CPU: a D interval spans CPUs, so per-CPU
//! storage cannot pair its edges. Lookups are lock-free RCU; locked updates
//! happen only on D transitions (CLAUDE.md §2).

use aya_ebpf::{
    macros::map,
    maps::{HashMap, PerCpuHashMap},
};

use crate::task::Task;

// include/linux/sched.h; NOLOAD excluded to skip TASK_IDLE kthreads.
const TASK_UNINTERRUPTIBLE: u32 = 0x2;
const TASK_NOLOAD: u32 = 0x400;

// tid → D-state switch-out timestamp.
#[map]
static SLEEP_SINCE: HashMap<u32, u64> = HashMap::with_max_entries(16384, 0);
// tgid → accumulated D-sleep nanoseconds (the counter user space diffs).
#[map]
static IOWAIT_NS: PerCpuHashMap<u32, u64> = PerCpuHashMap::with_max_entries(16384, 0);

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
pub(crate) fn wake_in(next: &Task, tid: u32, now: u64) {
    if tid == 0 {
        return;
    }
    let Some(since) = (unsafe { SLEEP_SINCE.get(tid) }).copied() else {
        return;
    };
    let _ = SLEEP_SINCE.remove(tid);
    let tgid = next.tgid();
    if tgid == 0 {
        return;
    }
    let total = (unsafe { IOWAIT_NS.get(tgid) }).copied().unwrap_or(0);
    let _ = IOWAIT_NS.insert(tgid, total.saturating_add(now.saturating_sub(since)), 0);
}

/// Stamp is per-thread; the accumulator is per-process, reaped on the leader.
#[inline(always)]
pub(crate) fn forget(tid: u32, tgid: u32) {
    let _ = SLEEP_SINCE.remove(tid);
    if tid == tgid {
        let _ = IOWAIT_NS.remove(tgid);
    }
}
