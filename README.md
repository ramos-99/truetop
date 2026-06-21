# truetop

Per-process Linux monitor built on eBPF raw tracepoints. CPU time and process
identity are collected entirely in-kernel (O(1) hotpaths). Memory (RSS) is the
one metric eBPF cannot read accurately on current kernels, so it falls back to
`/proc` — strictly for lack of an alternative, until the kernel exposes a usable
eBPF interface for it (see Memory). Block I/O latency per PID is planned.

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

The build script compiles eBPF programs automatically. To invoke the eBPF
build step explicitly:

```sh
cargo xtask build-ebpf
```

## Process names

Process identity (`comm`) is captured in-kernel on `sched_process_exec`, so
names of processes started while truetop runs cost nothing on the hotpath.
Processes that already existed at startup predate any event we can hook, so
their names are seeded **once from `/proc` at launch** — the only `/proc` access
in the tool. This startup backfill is planned to move to a `bpf_iter` task walk,
making truetop fully `/proc`-independent.

## Memory

RSS is read from `/proc/<pid>/statm` in user space, for the visible rows only
(the list is capped after sorting by CPU), so the cost is bounded regardless of
process count.

This is a fallback for lack of options, not a design preference. Since Linux 6.2
a process's RSS lives in a `percpu_counter`: the true value (what `top` shows) is
the global count plus the unflushed per-CPU deltas. Summing those from eBPF would
require walking `__percpu` pointers per online CPU — fragile, arch-specific, and
high-overhead — while the global count alone drifts from `top` by megabytes on
busy multi-threaded processes. No eBPF interface currently exposes the accurate
per-process value, so `/proc` is the only correct source today.

**When the kernel provides a usable interface** — a BPF helper or a stable
tracepoint carrying the summed value — **memory will move to eBPF like the rest.**
Until then this is the one metric not collected in-kernel.

## Overhead

`sched_switch` fires on every context switch. Per-event cost is O(1) and
nanosecond-level, but not zero. Benchmark on latency-sensitive hosts before
deploying.

## License

User-space: MIT OR Apache-2.0. eBPF code (`truetop-ebpf/`): GPL-2.0 OR MIT.
