# truetop

Per-process Linux monitor built on eBPF raw tracepoints. CPU time, I/O wait
(D-state time: which process is stuck on the disk, a column no procfs tool
shows), and process identity are collected entirely in-kernel (O(1) hotpaths).
Memory (RSS) is the one metric eBPF cannot read accurately on current kernels,
so it falls back to `/proc` until the kernel exposes a usable interface (see
Memory). Block-device latency per PID is planned.

## Benchmarks

truetop collects per-process CPU with O(1) syscalls per refresh, regardless of
process count. top and htop read one `/proc/<pid>` file per process, so their cost
grows linearly; truetop batch-reads a single kernel map. At 5,365 processes it
stays flat at ~780 syscalls per refresh against htop's ~11,959. The price is an
O(1) eBPF program on every context switch (see Overhead). Reproduce it all with
`cargo xtask bench`.

[![Read the benchmarks](https://img.shields.io/badge/read_the_benchmarks-method_%C2%B7_results_%C2%B7_kernel_cost-8250DF?style=for-the-badge)](bench/BENCHMARKS.md)

## Requirements

- Linux >= 5.10
- Kernel built with `CONFIG_DEBUG_INFO_BTF=y`
- `/sys/kernel/btf/vmlinux` present at runtime

```
rustup toolchain install stable
rustup toolchain install nightly --component rust-src
cargo install bpf-linker
```

## Build

```sh
cargo build --release
```

The build script compiles the eBPF programs automatically.

## Run

truetop loads eBPF programs, which needs root:

```sh
cargo xtask run            # build and run, elevating with sudo
# or run the built binary directly:
sudo ./target/release/truetop
```

## Tests

```sh
cargo test                 # unit tests: pure logic, no privileges
cargo xtask test           # eBPF integration tests: need root
```

The integration tests load the real programs on a live kernel, so they are
`#[ignore]`d under `cargo test` and run only via `cargo xtask test`.

## Process names

Process identity (`comm`) is captured in-kernel on `sched_process_exec`, so
names of processes started while truetop runs cost nothing on the hotpath.
Processes that already existed at startup predate any event we can hook, so
their names are seeded **once from `/proc` at launch**, the only `/proc` access
in the tool. This startup backfill is planned to move to a `bpf_iter` task walk,
making truetop fully `/proc`-independent.

## Memory

RSS is read from `/proc/<pid>/statm` in user space, for the visible rows only
(the list is capped after sorting by CPU), so the cost is bounded regardless of
process count.

This is a fallback for lack of options, not a design preference. Since Linux 6.2
a process's RSS lives in a `percpu_counter`: the true value (what `top` shows) is
the global count plus the unflushed per-CPU deltas. Summing those from eBPF would
require walking `__percpu` pointers per online CPU, which is fragile,
arch-specific, and high-overhead, while the global count alone drifts from `top`
by megabytes on busy multi-threaded processes. No eBPF interface currently exposes
the accurate per-process value, so `/proc` is the only correct source today.

**When the kernel provides a usable interface** (a BPF helper or a stable
tracepoint carrying the summed value), **memory will move to eBPF like the rest.**
Until then this is the one metric not collected in-kernel.

## Overhead

`sched_switch` fires on every context switch, so truetop's cost scales with the
context-switch rate, not the process count. Under `hackbench` (a context-switch
storm) it adds ~8% wall-clock on the reference machine, at low-microsecond cost
per switch; a normal system does orders of magnitude fewer switches and pays
proportionally less, well under 1%. The per-event cost is O(1) but not zero, so
benchmark on latency-sensitive hosts before deploying. See the hotpath benchmark
in [bench/BENCHMARKS.md](bench/BENCHMARKS.md).

## License

User-space: MIT OR Apache-2.0. eBPF code (`truetop-ebpf/`): GPL-2.0 OR MIT.
