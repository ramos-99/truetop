//! truetop kernel-space programs - one BPF object, split by concern so new
//! hooks slot in without ever touching the hotpath:
//!
//! - [`task`]      - shared CO-RE task introspection (injected field offsets),
//! - [`sched`]     - the `sched_switch` hotpath, fanned out to each metric,
//! - [`cpu`]       - on-CPU time accounting,
//! - [`iowait`]    - uninterruptible (D-state) sleep accounting,
//! - [`comm`]        - process identity, captured on the `exec` and `fork` tracepoints (cold path),
//! - [`lifecycle`]   - `sched_process_exit`: drop transient state, announce the exit,
//! - [`exit_notify`] - the exit ring buffer user space reaps the accounting from.
//!
//! Memory is deliberately *not* here - see `truetop`'s collector and the README:
//! since Linux 6.2 RSS lives in a `percpu_counter` whose exact value can't be
//! summed from eBPF cheaply, so it is read from `/proc` in user space.

#![no_std]
#![no_main]

mod comm;
mod cpu;
mod exit_notify;
mod iowait;
mod lifecycle;
mod sched;
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
