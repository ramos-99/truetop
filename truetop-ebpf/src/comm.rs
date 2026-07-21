//! Process identity capture. `comm` is written only on the cold `exec` and
//! `fork` paths (and dropped on `exit`), never on `sched_switch`, so the hotpath
//! stays string-free (CLAUDE.md §2/§6).
//!
//! `exec` alone would miss every process that forks and never calls `execve` -
//! how PostgreSQL backends, nginx workers and Redis children are spawned -
//! leaving them unnamed. The kernel copies the parent's `comm` into the child in
//! `copy_process`, before `sched_process_fork` fires, so the fork hook records
//! that inherited name (read from `current`, which is still the parent); a later
//! `exec` overwrites it with the real one. Forks are orders of magnitude rarer
//! than context switches, so both writes stay off the hot path.
//!
//! This map is intentionally a global `HashMap`, not per-CPU: the §2 per-CPU
//! rule targets hotpath spinlock contention, which a cold-path write does not
//! incur, and a per-process name is not per-CPU data - only the core that ran
//! the `exec` would hold a per-CPU copy.

use aya_ebpf::{
    helpers::{TASK_COMM_LEN, bpf_get_current_comm},
    macros::{map, raw_tracepoint},
    maps::HashMap,
    programs::RawTracePointContext,
};
use truetop_common::COMM_LEN;

use crate::task::Task;

// The shared wire width must match the kernel's; lock it at compile time.
const _: () = assert!(COMM_LEN == TASK_COMM_LEN);

// tgid → comm (NUL-padded).
#[map]
static COMM_MAP: HashMap<u32, [u8; COMM_LEN]> = HashMap::with_max_entries(16384, 0);

#[raw_tracepoint(tracepoint = "sched_process_exec")]
pub fn sched_process_exec(ctx: RawTracePointContext) -> i32 {
    // args: (*p, old_pid, *bprm); `comm` already holds the new program name.
    let tgid = Task::arg(&ctx, 0).tgid();
    if tgid != 0
        && let Ok(comm) = bpf_get_current_comm()
    {
        let _ = COMM_MAP.insert(tgid, comm, 0);
    }
    0
}

#[raw_tracepoint(tracepoint = "sched_process_fork")]
pub fn sched_process_fork(ctx: RawTracePointContext) -> i32 {
    // args: (*parent, *child); `current` is the parent, whose `comm` the child
    // has already inherited.
    let tgid = Task::arg(&ctx, 1).tgid();
    if tgid != 0
        && let Ok(comm) = bpf_get_current_comm()
    {
        let _ = COMM_MAP.insert(tgid, comm, 0);
    }
    0
}

/// Drop a dead process's name; called by the shared exit hook on the leader.
#[inline(always)]
pub(crate) fn forget(tid: u32, tgid: u32) {
    if tid == tgid {
        let _ = COMM_MAP.remove(tgid);
    }
}
