# Contributing

Patches, bug reports and kernel-coverage reports are all welcome. This file
covers the toolchain, the test layers and what a reviewable change looks like.

## Prerequisites

```sh
rustup toolchain install stable
rustup toolchain install nightly --component rust-src
cargo install bpf-linker --locked
```

Nightly is not optional: `truetop/build.rs` compiles the eBPF object with `-Z
build-std`, and `rustfmt.toml` uses unstable options. The user-space crates
build on stable. Loading anything needs `CONFIG_DEBUG_INFO_BTF=y` and either
root or `CAP_BPF` plus `CAP_PERFMON`.

## Layout

- `truetop/` binary: BTF offset resolution, loader, collector, ratatui UI.
- `truetop-ebpf/` kernel programs, `no_std`, one BPF object.
- `truetop-common/` `#[repr(C)]` types shared across the FFI boundary.
- `bench/` load generators, criterion cost models, benchmark scripts.
- `xtask/` task runner, so privileged steps need no global cargo runner.

Kernel-side code stays O(1) per event and stores counters and timestamps only;
anything that has to iterate belongs in the collector. Kernel struct fields are
read through `bpf_core_read!` or an offset injected from BTF at load, never by
dereferencing. Anything both sides read is `#[repr(C)]` and lives in
`truetop-common`, defined once.

`truetop-ebpf` does not compile for the host, so host-wide cargo commands need
`--workspace --exclude truetop-ebpf`. Building `truetop` builds it for you.

## Build and test

```sh
cargo build
cargo xtask run [args...]                       # build and run, elevating with sudo

cargo test                                      # unit and pure tests, no root
cargo test -p truetop --features btf-fixtures   # BTF parser against real kernel blobs
cargo xtask test                                # eBPF integration tests, as root
cargo xtask test cross_kernel                   # one target only
```

The tests in `truetop/tests/` load real programs, so they need root and a live
kernel and are `#[ignore]`d to keep `cargo test` fast and unprivileged. `cargo
xtask test` builds them, then re-runs each binary under `sudo -E` with
`--ignored --test-threads=1`; serial is deliberate, since the oracles compare
against `/proc` and would otherwise fight for the CPU.

Keep the split that already exists there: assertions on a magnitude (CPU% versus
`/proc`, I/O wait versus `delayacct`) need real hardware and stay on the host,
while `cross_kernel.rs` asserts only that the programs verify, attach and
collect, so it can run anywhere.

BTF fixtures under `truetop/testdata/` are checksum-pinned BTFHub blobs;
refresh them with `cargo xtask update-btf-fixtures`.

## Kernel matrix

`.github/workflows/vm-matrix.yml` boots 5.15 through 6.18 under vmtest, in the
Arch and Fedora configs (the `-default` variants ship no BTF), and runs
`cross_kernel` on each. Anything touching BTF offsets, verifier-visible code or
tracepoint arguments has to pass the matrix; your own kernel is one data point.
Locally, with qemu and `vmtest` installed:

```sh
cargo build -p xtask && cargo test -p truetop --no-run   # the guest has no network
curl -fL -o bzImage https://github.com/danobi/vmtest/releases/download/test_assets/bzImage-v6.12-archlinux
vmtest -k bzImage "export PATH='$PATH' CARGO_NET_OFFLINE=true && cargo xtask test cross_kernel"
```

## Benchmarks

```sh
cargo xtask bench                    # micro, macro and hotpath
cargo xtask bench hotpath selfcpu    # any subset, in any order
cargo xtask bench --no-prep [...]    # skip CPU tuning, for VMs and CI
```

The run pins the governor to `performance`, disables turbo and restores both on
exit. `selfcpu` and `switch` are opt-in, being slow and needing htop and btop
installed; `macro` needs `strace`, `hotpath` needs `bpftool`, `jq`, `hackbench`
and `perf`. Plots want python with matplotlib and are skipped without it.
Results land in `bench/results/`, gitignored apart from the tracked SVGs.

Re-measure with `bench hotpath` after any change to the `sched_switch` path,
before quoting a figure anywhere, and quote the machine and clocksource with it:
`bench/results/env.txt` records both, and the per-event cost roughly triples on
`hpet`. Method and current results are in `bench/BENCHMARKS.md`.

## Style

```sh
cargo +nightly fmt --all
cargo clippy --workspace --exclude truetop-ebpf --all-targets -- -D warnings
```

Both run in CI and both must be clean. Otherwise match the surrounding code:
modules open with a doc comment saying why they exist that way, and inline
comments carry the reasoning rather than restating the line below. A comment
that asserts a mechanism the code no longer has is worse than none, so delete it
in the same commit that removes the mechanism.

## Commits and pull requests

One logical change per commit, squashed before review, with review addressed by
rebasing rather than fixup commits. The subject names the area it touched, then
says what the change does:

```
truetop-ebpf: bill in-flight time so a never-preempted thread isn't 0%

sched_switch only bills a thread's slice at switch-out, so one that
runs a whole tick without being preempted was never charged. Track
who's on each CPU and since when, and add that slice at read time.
```

The area is the crate (`truetop`, `truetop-ebpf`, `truetop-common`, `bench`,
`xtask`), or the module inside it when that says more (`ui`, `collector`, `btf`,
`iowait`), or `ci`, `docs`, `readme` for the rest. Nothing classifies the change
as a feature or a fix; the diff already does that. Subject in lowercase and the
imperative under 72 characters, body wrapped at 72 explaining why, `Fixes: #N`
or `Refs: #N` at the end where it applies.

A behaviour change and its regression test land in the same commit. Run fmt,
clippy, `cargo test` and `cargo xtask test` before opening the pull request, and
if a change moves a published number, include the before and after.

## Bug reports

For a CO-RE tool a report without its environment is not actionable. Include
`uname -a`, the distro and architecture, whether `CONFIG_DEBUG_INFO_BTF` is set,
how you ran it (root, `setcap`, container, VM), and the exact output. If the
complaint is a wrong number, include the same window from whatever you compared
against. Security issues go through `SECURITY.md`, not the issue tracker.

If you work with an agentic AI assistant, point it at `CLAUDE.md`, which states
the same architectural constraints in a form it will follow.

## Licensing

Contributions ship under the licences already in the tree: `MIT OR Apache-2.0`
for the user-space crates, `GPL-2.0 OR MIT` for `truetop-ebpf/`, matching the
licence that object declares to the kernel verifier. There is no CLA.
