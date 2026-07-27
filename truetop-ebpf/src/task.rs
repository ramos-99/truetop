//! Shared CO-RE task introspection used by the scheduler and lifecycle hooks.

use aya_ebpf::{Global, helpers::bpf_probe_read_kernel, programs::RawTracePointContext};

// task_struct field byte offsets, resolved from kernel BTF and injected at load
// (see user-space btf.rs). Portable stand-in for CO-RE field relocation.
#[unsafe(no_mangle)]
static PID_OFFSET: Global<u32> = Global::new(0);
#[unsafe(no_mangle)]
static TGID_OFFSET: Global<u32> = Global::new(0);
#[unsafe(no_mangle)]
static STATE_OFFSET: Global<u32> = Global::new(0);

/// A scheduler `task_struct` pointer taken from a tracepoint argument.
pub struct Task(*const u8);

impl Task {
    #[inline(always)]
    pub fn arg(ctx: &RawTracePointContext, n: usize) -> Self {
        Self(ctx.arg(n))
    }

    /// The thread id (the scheduler's pid); 0 means idle or unreadable.
    #[inline(always)]
    pub fn pid(&self) -> u32 {
        self.read_u32(PID_OFFSET.load())
    }

    /// `pid` (thread id) and `tgid` (process id) in a single probe-read. In
    /// `task_struct` these are two adjacent `pid_t`s - `pid_t pid; pid_t tgid;`:
    /// <https://elixir.bootlin.com/linux/v6.12/source/include/linux/sched.h#L1018>
    /// - so an 8-byte load at `PID_OFFSET` yields both and halves the hotpath
    /// reads. User space verifies the adjacency against the running kernel's BTF
    /// before load (the assert in `load_ebpf`). `pid` sits in the word's low
    /// half on a little-endian build (`bpfel`), high half on big-endian
    /// (`bpfeb`, e.g. s390x) - `aya-build` picks the BPF target to match.
    #[inline(always)]
    pub fn pid_and_tgid(&self) -> (u32, u32) {
        if self.0.is_null() {
            return (0, 0);
        }
        let field = unsafe { self.0.add(PID_OFFSET.load() as usize) } as *const u64;
        let word = unsafe { bpf_probe_read_kernel(field) }.unwrap_or(0);
        if cfg!(target_endian = "big") {
            ((word >> 32) as u32, word as u32)
        } else {
            (word as u32, (word >> 32) as u32)
        }
    }

    /// The thread-group id (the process id users see).
    #[inline(always)]
    pub fn tgid(&self) -> u32 {
        self.read_u32(TGID_OFFSET.load())
    }

    /// The scheduler run state (`__state`; `state` before 5.14 — user space
    /// points the offset at the meaningful word either way).
    #[inline(always)]
    pub fn state(&self) -> u32 {
        self.read_u32(STATE_OFFSET.load())
    }

    /// Probe-read a `pid_t`-sized field at `offset`; 0 on a null task or fault.
    #[inline(always)]
    fn read_u32(&self, offset: u32) -> u32 {
        if self.0.is_null() {
            return 0;
        }
        let field = unsafe { self.0.add(offset as usize) } as *const u32;
        unsafe { bpf_probe_read_kernel(field) }.unwrap_or(0)
    }
}
