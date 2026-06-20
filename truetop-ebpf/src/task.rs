//! Shared task introspection. The CO-RE field read every hook relies on.

use aya_ebpf::{Global, helpers::bpf_probe_read_kernel, programs::RawTracePointContext};

// task_struct::pid byte offset, resolved from kernel BTF and injected at load
// (see user-space btf.rs). Portable stand-in for CO-RE field relocation.
#[unsafe(no_mangle)]
static PID_OFFSET: Global<u32> = Global::new(0);

/// A scheduler `task_struct` pointer taken from a tracepoint argument.
pub struct Task(*const u8);

impl Task {
    #[inline(always)]
    pub fn arg(ctx: &RawTracePointContext, n: usize) -> Self {
        Self(ctx.arg(n))
    }

    /// `pid` read through the injected offset; 0 means idle or unreadable.
    #[inline(always)]
    pub fn pid(&self) -> u32 {
        if self.0.is_null() {
            return 0;
        }
        let field = unsafe { self.0.add(PID_OFFSET.load() as usize) } as *const i32;
        unsafe { bpf_probe_read_kernel(field) }
            .map(|pid| pid as u32)
            .unwrap_or(0)
    }
}
