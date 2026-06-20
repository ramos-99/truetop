//! CPU utilisation via `sched_switch` (CLAUDE.md §2). Per-CPU, add-only
//! counters; user space derives percentages from cross-tick deltas.

use aya_ebpf::{
    helpers::bpf_ktime_get_ns,
    macros::{map, raw_tracepoint},
    maps::PerCpuHashMap,
    programs::RawTracePointContext,
};

use crate::task::Task;

// pid → last schedule-in timestamp (scratch).
#[map]
static START_TIME: PerCpuHashMap<u32, u64> = PerCpuHashMap::with_max_entries(16384, 0);
// pid → accumulated on-CPU nanoseconds (the counter user space diffs).
#[map]
static CPU_NS: PerCpuHashMap<u32, u64> = PerCpuHashMap::with_max_entries(16384, 0);

#[raw_tracepoint(tracepoint = "sched_switch")]
pub fn sched_switch(ctx: RawTracePointContext) -> i32 {
    // args: (bool preempt, *prev, *next, [prev_state]).
    let now = unsafe { bpf_ktime_get_ns() };
    charge_out(Task::arg(&ctx, 1).pid(), now);
    mark_in(Task::arg(&ctx, 2).pid(), now);
    0
}

/// Drop a terminated pid's entries so the maps don't accumulate the dead
/// (CLAUDE.md §2). Each subsystem owns the cleanup of its own maps.
#[raw_tracepoint(tracepoint = "sched_process_exit")]
pub fn sched_process_exit(ctx: RawTracePointContext) -> i32 {
    // args: (*p).
    let pid = Task::arg(&ctx, 0).pid();
    if pid != 0 {
        let _ = START_TIME.remove(pid);
        let _ = CPU_NS.remove(pid);
    }
    0
}

/// Add the slice the outgoing task just ran to its running total.
#[inline(always)]
fn charge_out(pid: u32, now: u64) {
    if pid == 0 {
        return;
    }
    let Some(start) = (unsafe { START_TIME.get(pid) }).copied() else {
        return;
    };
    let total = (unsafe { CPU_NS.get(pid) }).copied().unwrap_or(0);
    let _ = CPU_NS.insert(pid, total.saturating_add(now.saturating_sub(start)), 0);
}

/// Stamp the incoming task's schedule-in time.
#[inline(always)]
fn mark_in(pid: u32, now: u64) {
    if pid == 0 {
        return;
    }
    let _ = START_TIME.insert(pid, now, 0);
}
