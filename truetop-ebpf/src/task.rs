//! Shared CO-RE task introspection used by the scheduler and lifecycle hooks.

use aya_ebpf::{Global, helpers::bpf_probe_read_kernel, programs::RawTracePointContext};

// task_struct field byte offsets, resolved from kernel BTF and injected at load
// (see user-space btf.rs). Portable stand-in for CO-RE field relocation.
#[unsafe(no_mangle)]
static PID_OFFSET: Global<u32> = Global::new(0);
#[unsafe(no_mangle)]
static TGID_OFFSET: Global<u32> = Global::new(0);

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

    /// The thread-group id (the process id users see).
    #[inline(always)]
    pub fn tgid(&self) -> u32 {
        self.read_u32(TGID_OFFSET.load())
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
