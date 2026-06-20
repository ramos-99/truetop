//! Constants and types shared across the eBPF ↔ user-space boundary
//! (CLAUDE.md §4) so both sides agree on the wire layout.
#![no_std]

/// Width of the kernel's `comm` field (`TASK_COMM_LEN`) — the value size of the
/// tgid→name map shared between the eBPF capture and the user-space reader.
pub const COMM_LEN: usize = 16;
