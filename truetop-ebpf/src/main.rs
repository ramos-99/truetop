#![no_std]
#![no_main]

use aya_ebpf::{macros::raw_tracepoint, programs::RawTracePointContext};
use aya_log_ebpf::info;

#[raw_tracepoint(tracepoint = "sched_switch")]
pub fn truetop(ctx: RawTracePointContext) -> i32 {
    match try_truetop(ctx) {
        Ok(ret) => ret,
        Err(ret) => ret,
    }
}

fn try_truetop(ctx: RawTracePointContext) -> Result<i32, i32> {
    info!(&ctx, "tracepoint sched_switch called");
    Ok(0)
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[unsafe(link_section = "license")]
#[unsafe(no_mangle)]
static LICENSE: [u8; 13] = *b"Dual MIT/GPL\0";
