//! Constants and types shared across the eBPF ↔ user-space boundary, every one
//! of them `#[repr(C)]` so both sides agree on the wire layout.
#![no_std]

/// Width of the kernel's `comm` field (`TASK_COMM_LEN`) - the value size of the
/// tgid→name map shared between the eBPF capture and the user-space reader.
pub const COMM_LEN: usize = 16;

/// What the kernel captures about a process: its program name and the uid that
/// owns it. Both are written on the cold `exec` and `fork` paths, so neither
/// costs user space a `/proc` read per tick.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub comm: [u8; COMM_LEN],
    pub uid: u32,
}

// SAFETY: `#[repr(C)]`, no padding, no pointers - safe to read as bytes.
#[cfg(feature = "user")]
unsafe impl aya::Pod for Identity {}

/// A process that has ended, announced over the exit ring buffer.
///
/// It carries identity only. The counters live in per-CPU maps, and an eBPF
/// program can read no slot but its own CPU's, so the totals a departing process
/// leaves behind can only be summed in user space; this says who to look for.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExitEvent {
    pub tgid: u32,
}

impl ExitEvent {
    /// Decode one record off the ring, or `None` if it is short.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let tgid = bytes.get(..size_of::<u32>())?.try_into().ok()?;
        Some(Self {
            tgid: u32::from_ne_bytes(tgid),
        })
    }
}
