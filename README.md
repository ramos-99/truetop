# truetop

Linux system monitor using eBPF raw tracepoints. Per-PID CPU, memory, and
block I/O latency without procfs polling.

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

## Overhead

`sched_switch` fires on every context switch. Per-event cost is O(1) and
nanosecond-level, but not zero. Benchmark on latency-sensitive hosts before
deploying.

## License

User-space: MIT OR Apache-2.0. eBPF code (`truetop-ebpf/`): GPL-2.0 OR MIT.
