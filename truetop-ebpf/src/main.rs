//! truetop kernel-space programs — one BPF object, split by concern so new
//! hooks slot in without ever touching the hotpath:
//!
//! - [`task`]      — shared CO-RE task introspection (injected pid/tgid offsets),
//! - [`cpu`]       — CPU time via `sched_switch` (the hotpath),
//! - [`comm`]      — process identity via `sched_process_exec` (cold path),
//! - [`lifecycle`] — `sched_process_exit` cleanup fanned out to each subsystem.

#![no_std]
#![no_main]

mod comm;
mod cpu;
mod lifecycle;
mod task;

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

// Required for the verifier to load programs that call GPL-only helpers.
#[unsafe(link_section = "license")]
#[unsafe(no_mangle)]
static LICENSE: [u8; 13] = *b"Dual MIT/GPL\0";
