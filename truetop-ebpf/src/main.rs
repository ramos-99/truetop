#![no_std]
#![no_main]

use aya_ebpf::{
    Global,
    helpers::bpf_probe_read_kernel,
    macros::{map, raw_tracepoint},
    maps::Array,
    programs::RawTracePointContext,
};

/// Byte offset of `task_struct::pid`, injected at load time once user space
/// resolves it against the running kernel's BTF (see `btf.rs`). This is our
/// portable stand-in for libbpf CO-RE field relocation, which the Rust eBPF
/// toolchain does not emit — one binary then adapts to any kernel/arch.
#[unsafe(no_mangle)]
static PID_OFFSET: Global<u32> = Global::new(0);

/// Smoke-test probe: the most recent scheduled-in pid read via `PID_OFFSET`.
/// Temporary — replaced by the per-CPU accounting maps in the next commit.
#[map]
static LAST_PID: Array<u32> = Array::with_max_entries(1, 0);

#[raw_tracepoint(tracepoint = "sched_switch")]
pub fn sched_switch(ctx: RawTracePointContext) -> i32 {
    // raw sched_switch args: (bool preempt, *prev, *next, [prev_state]).
    let next: *const u8 = ctx.arg(2);
    let pid = unsafe { read_pid(next) };
    if let Some(slot) = LAST_PID.get_ptr_mut(0) {
        unsafe { *slot = pid };
    }
    0
}

/// CO-RE field read: `*(task + PID_OFFSET)` via a safe kernel probe read.
#[inline(always)]
unsafe fn read_pid(task: *const u8) -> u32 {
    if task.is_null() {
        return 0;
    }
    let off = PID_OFFSET.load() as usize;
    match unsafe { bpf_probe_read_kernel(task.add(off) as *const i32) } {
        Ok(pid) => pid as u32,
        Err(_) => 0,
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[unsafe(link_section = "license")]
#[unsafe(no_mangle)]
static LICENSE: [u8; 13] = *b"Dual MIT/GPL\0";
