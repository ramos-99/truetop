<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/truetop-logo.png">
  <source media="(prefers-color-scheme: light)" srcset="assets/truetop-logo-light.png">
  <img alt="truetop" src="assets/truetop-logo-light.png" width="380">
</picture>

**Per-process Linux monitor built on eBPF.**

_The I/O-wait column `top`, `htop` and `btop` don't have._

**Pre-release.** Builds from source, no packaged binaries yet - see [Install](#install).

[![CI](https://img.shields.io/github/actions/workflow/status/ramos-99/truetop/ci.yml?style=flat-square&logo=github&label=CI)](https://github.com/ramos-99/truetop/actions/workflows/ci.yml)
[![kernel matrix](https://img.shields.io/github/actions/workflow/status/ramos-99/truetop/vm-matrix.yml?style=flat-square&logo=linux&logoColor=white&label=kernels%205.15%E2%80%936.18)](https://github.com/ramos-99/truetop/actions/workflows/vm-matrix.yml)
[![eBPF CO-RE](https://img.shields.io/badge/eBPF-CO--RE-F76800?style=flat-square&logoColor=white)](#how-it-works)
[![License](https://img.shields.io/badge/License-MIT%20or%20Apache--2.0-2EA043?style=flat-square)](#license)
[![Contributing](https://img.shields.io/badge/Contributing-guide-2B3137?style=flat-square&logo=git&logoColor=white)](CONTRIBUTING.md)
[![Security policy](https://img.shields.io/badge/Security-policy-2B3137?style=flat-square&logo=letsencrypt&logoColor=white)](SECURITY.md)

<img src="assets/demo.gif" alt="truetop sorting by I/O wait: fio readers blocked on the disk light up red" width="820">

</div>

---

truetop shows per-process CPU, memory, and one column `top`, `htop` and `btop`
do not have: how long each process sat blocked on storage. Everything except
memory is collected inside the kernel by O(1)-per-event eBPF programs and drained
in batched reads rather than one syscall per process, so collection cost tracks
the context-switch rate rather than how many processes are running.

## Contents

[Why](#why) · [Comparison](#comparison) · [Features](#features) · [Requirements](#requirements) · [Install](#install) · [Usage](#usage) · [How it works](#how-it-works) · [Benchmarks](#benchmarks) · [Overhead](#overhead) · [What reads /proc](#what-still-reads-proc) · [Roadmap](#roadmap) · [Contributing](#contributing) · [Security](#security) · [License](#license)

## Why

The premise: build a `top`-class monitor almost entirely in eBPF instead of
procfs, and use that to expose numbers procfs cannot reach at all, not only to
collect the same numbers cheaper. Per-process I/O wait is the first case: `top`,
`htop` and `btop` all read process state from `/proc`, which carries no
per-process uninterruptible-sleep counter to read. eBPF sees the scheduler
directly, so it can show it. Anything else that lives at the scheduler or block
layer and never made it into procfs is a candidate for the same treatment; block
device latency per process is next, see [Roadmap](#roadmap).

## Comparison

| | truetop | top | htop | btop | iotop |
| --- | :---: | :---: | :---: | :---: | :---: |
| Per-process I/O wait (time blocked) | yes | no | no | no | gated <sup>1</sup> |
| Per-process I/O throughput (bytes/s) | no | no | yes | no | yes |
| CPU and memory per process | yes | yes | yes | yes | no |
| Process tree | no | yes | yes | yes | no |
| Kill, signal, renice | no | yes | yes | yes | no |
| Per-thread rows | no | yes | yes | no | yes |
| cgroup column | no | no | yes | no | no |
| Reads from | eBPF | `/proc` | `/proc` | `/proc` | netlink |
| Privileges | `CAP_BPF`, `CAP_PERFMON` | none | none | none | root or `CAP_NET_ADMIN` |
| Kernel needs | 5.15+ with BTF | any | any | any | 4 configs + sysctl <sup>2</sup> |

truetop does not manage processes: there is no tree, no signal, no renice.

<sup>1</sup> `iotop`'s `IO>` column comes from delay accounting, which has been
off by default since 5.14: it needs `sysctl kernel.task_delayacct=1` or the
`delayacct` boot parameter, and enabling it costs performance system-wide.
truetop reads the same intervals from the scheduler with nothing to switch on.

<sup>2</sup> `CONFIG_TASK_DELAY_ACCT`, `CONFIG_TASK_IO_ACCOUNTING`,
`CONFIG_TASKSTATS` and `CONFIG_VM_EVENT_COUNTERS`.

## Features

- **Per-process I/O wait.** The time each process spent in uninterruptible
  (D-state) sleep, charged to the task that actually blocked. `top`, `htop` and
  `btop` do not show it per process at all; `iotop` derives a related number from
  `delayacct` over netlink, behind the sysctl above. truetop reads it from the
  scheduler, in-kernel, next to everything else.

  It is a diagnostic rather than a dashboard: on an idle machine the column
  stays empty. A kernel worker high in it is deferred writeback being flushed;
  one of your own processes high in it is a synchronous stall, which is the case
  the other monitors leave you to guess at.

- **Batched collection.** CPU and I/O wait accumulate on `sched_switch`, and each
  refresh drains the maps with `bpf_map_lookup_batch`, thousands of processes per
  syscall instead of one syscall per process. The process-keyed maps hold 65,536
  entries by default on ordinary hardware; past capacity they evict the coldest
  one rather than refusing new ones, and the status line says so.

- **One binary across kernels.** `task_struct` offsets are resolved from the
  kernel's own BTF at load and injected as constants, without libbpf. CI runs
  the programs on live kernels 5.15 through 6.18, on Arch and Fedora configs.

- **Measured overhead.** The `sched_switch` hook runs on every context switch,
  so the cost tracks the switch rate rather than the process count. The figures,
  and the crossover past which htop is cheaper, are in [Overhead](#overhead).

## Requirements

- Linux 5.15 or newer on x86-64, the range CI runs the programs on (5.15 through
  6.18). The pre-5.14 `state`-field path is still in the code, so 5.10 through
  5.14 may work, but as of today that is neither tested nor guaranteed. aarch64
  compiles but is not tested at runtime.
- A kernel built with `CONFIG_DEBUG_INFO_BTF=y`, so `/sys/kernel/btf/vmlinux`
  exists at runtime. Every mainstream distro ships this, and truetop aborts with
  a message naming the option if it is missing.
- `CAP_BPF` and `CAP_PERFMON`, or root, to load the programs.

## Install

There is no published binary yet. The build compiles the eBPF object, so it
needs the nightly toolchain and `bpf-linker`:

```sh
rustup toolchain install stable
rustup toolchain install nightly-2026-08-25 --component rust-src
cargo binstall bpf-linker          # or: cargo install cargo-binstall, first
```

That nightly is pinned in `truetop-ebpf/Cargo.toml`, which is what the build
uses: `bpf-linker` carries its own LLVM and cannot read bitcode from a newer
rustc, so the two move together.

Then either build the repository:

```sh
git clone https://github.com/ramos-99/truetop
cd truetop
cargo build --release
```

or install straight from git:

```sh
cargo install --locked --git https://github.com/ramos-99/truetop truetop
```

`--locked` is not optional: without it `cargo install --git` ignores `Cargo.lock`
and resolves the dependency graph afresh, so the build you get is not the build
CI tested.

Prebuilt releases and an AUR package are on the [roadmap](#roadmap).

## Usage

Loading eBPF needs privileges, so run as root or grant the binary the two
capabilities:

```sh
sudo ./target/release/truetop
sudo setcap cap_bpf,cap_perfmon+ep ./target/release/truetop   # then run it unprivileged
cargo xtask run                                               # build and run, elevating with sudo
```

| key | action |
| --- | --- |
| `q`, `Esc` | quit |
| `↑` `↓`, `k` `j` | move the selection |
| `PgUp`, `PgDn` | move a page |
| `Home`, `End` | first, last row |
| `c`, `m`, `i` | sort by CPU, memory, I/O wait; press again to reverse |
| `/` | filter by program name or pid |
| `space` | pause |

| flag | what it does |
| --- | --- |
| `--background <dark\|light\|auto>` | row tints for the terminal's background. `auto`, the default, asks the terminal over OSC 11 and falls back to dark where nothing answers, as under tmux |
| `--max-processes <N>` | entries in the kernel accounting maps. Raise it if the status line reports `at capacity` |
| `--bench <TICKS>` | run the collector headless for a fixed number of ticks, which is what the benchmarks drive |
| `--help`, `--version` | answer without privileges |

Columns are `Pid`, `User`, `Program`, `Cpu%`, `Mem` and `IO Wait`. Each snapshot
carries the top 256 processes by CPU and the top 256 by I/O wait - up to 512
rows once the two are merged - so drawing does not grow with the process count
either.

The `--max-processes` default is derived from the CPU count rather than fixed,
because the maps are preallocated and a per-CPU one costs `N × 8 bytes × CPUs`:
the full 65,536 up to 64 CPUs, tapering to 16,384 above that.

## How it works

Four raw tracepoints and one `fentry` hook, none of which reads `/proc`:

| hook | captures | why it exists |
| --- | --- | --- |
| `sched_switch` | on-CPU time, and a timestamp when a task leaves the CPU in uninterruptible sleep, charged when it comes back | `Cpu%` and `IO Wait` |
| `sched_process_exec` | program name and owning user | the baseline name, which the fork hook backfills for processes that never reach it |
| `sched_process_fork` | the same, inherited from the parent | names children that never call `execve`, such as PostgreSQL backends and nginx workers |
| `commit_creds` | the uid after a process changes it | a worker that drops privileges shows the user it runs as rather than the one that started it. The one hook that is not a tracepoint, because the kernel offers none for credential changes |
| `sched_process_exit` | the departure, on a ring buffer | user space reads the final totals on its next refresh, so a process that ran and ended between two refreshes is still charged its time, and only then is the entry deleted |

A process that both starts and ends within a single refresh is the exception:
with no earlier sample to subtract from, that interval's time is not attributed
to it.

`task_struct` field offsets are read from the kernel's own BTF at load time and
injected as constants, which is what lets one binary span kernel versions
without libbpf.

<details>
<summary><b>The two threads in user space</b></summary>

<br>

User space runs two threads over an [`arc-swap`](https://docs.rs/arc-swap). A
collector wakes every second, drains every per-CPU map in one
`bpf_map_lookup_batch`, computes deltas against the previous tick, and publishes
an immutable snapshot. The [ratatui](https://ratatui.rs) renderer loads that
snapshot without locking and formats only the rows it draws. Kernel-side work is
O(1) per event and stores counters and timestamps only; all aggregation happens
in user space.

</details>

## Benchmarks

<div align="center">
<img src="bench/results/scaling.svg" alt="CPU collection cost versus process count, for truetop, top and htop" width="720">
</div>

Reproduce these with `cargo xtask bench`. Re-measured 2026-07-29 on the reference
machine named in [bench/BENCHMARKS.md](bench/BENCHMARKS.md), which also records
what moved since these were first published and why.

| metric | truetop | htop / btop |
| --- | --- | --- |
| CPU at ~7,050 processes | 1.3% of a core, flat | 33.6% / 23.6%, climbing |
| CPU under ~3,260 forks/s churn | 1.3% of a core | 2.7% (htop) |
| data syscalls per refresh at ~5,350 | ~3,970 | ~12,900 (htop) |
| per-process collection work | ~26 ns | ~12.3 µs (procfs) |
| RSS, this process, at ~7,050 processes | ~33 MiB, flat | ~19 / ~17 MiB, climbing |

RSS is the one line where truetop starts behind: htop and btop start smaller and
grow with process count, crossing truetop's flat floor somewhere past the range
tested here. Detail in [bench/BENCHMARKS.md](bench/BENCHMARKS.md#selfcpu-the-tools-own-cost).

## Overhead

truetop's cost scales with the context-switch rate rather than the process
count, so a switch-heavy machine with few processes costs more rather than
less.

<details>
<summary><b>Per-event cost and the crossover</b></summary>

<br>

`sched_switch` fires on every context switch. The per-event program is O(1) and
measured at a median of ~502 ns on the reference machine (turbo off, `tsc`
clocksource - both move this figure, see
[bench/BENCHMARKS.md](bench/BENCHMARKS.md)). Under a `hackbench`
context-switch storm that cost ~+2% wall clock; an ordinary system does orders of
magnitude fewer switches and pays well under 1%.

Roughly 20-45 ns of that is the accounting maps being LRU rather than a fixed
hash, so a full map evicts its coldest entry instead of silently refusing new
processes - see [Features](#features).

truetop is cheaper than htop only below some switch rate that rises with process
count. The exact crossover has not been re-measured against today's per-event
cost - see
[bench/BENCHMARKS.md](bench/BENCHMARKS.md#switch-cost-under-a-context-switch-storm).
The figure also tracks the clocksource, since `bpf_ktime_get_ns` runs per event
and roughly triples on `hpet`. Benchmark before deploying on latency-sensitive
hosts.

</details>

## What still reads `/proc`

Everything about a process's *behaviour* comes from the kernel: CPU time, I/O
wait, and the name and owning user of anything that starts while truetop runs.
What does not is listed here, rather than left to be found under `strace`:

| what | when | why |
| --- | --- | --- |
| RSS, from `statm` | per visible row, per refresh | no eBPF interface gives an exact figure, see below |
| names and uids of processes that predate truetop | once, at startup | a `bpf_iter` task walk would do it in-kernel, see [Roadmap](#roadmap) |
| `meminfo` and `loadavg`, for the header | two small files, per refresh | machine-wide, so the cost is constant, not per process |
| hostname, CPU model, total memory | once, at startup | the same, and they do not change |

Only the first is permanent. The second is the last O(N) procfs scan in the tool
and is on its way out. The rest is two files a second, whether the machine runs
fifty processes or fifty thousand.

RSS comes from `/proc/<pid>/statm` in user space, read only for the rows on
screen, so the cost stays bounded regardless of process count. This is a fallback
rather than a preference. Since Linux 6.2 a process's RSS lives in a
`percpu_counter`, and no eBPF interface exposes the summed value; the global
count on its own drifts from `top` by megabytes on busy multi-threaded processes.
If the kernel grows a helper or a tracepoint carrying the total, memory moves
in-kernel with everything else.

## Roadmap

- Per-PID block device latency from `block_rq_issue` and `block_rq_complete`.
- A `bpf_iter` task walk to replace the one-time `/proc` sweep that names
  processes which already existed at startup. That sweep is a stopgap: it is the
  last O(N) procfs scan left in truetop, and the only reason startup cost tracks
  the process count rather than being flat like everything else here. A task
  iterator reads the same names and uids from `task_struct` in one kernel-side
  pass. [aya](https://aya-rs.dev) already loads and attaches iterators from user
  space; `aya-ebpf` has no iterator program type yet, so the kernel half would
  have to be hand-rolled against a raw context. **Contributions adding iterator
  support to aya would be very welcome**, and would reduce this to a small change
  here.
- Memory in-kernel, once an accurate per-process interface exists.
- Kernels 5.10 through 5.13 in CI, which take the pre-5.14 `state` field path,
  and aarch64 at runtime.
- Prebuilt releases and an AUR package.

## Contributing

The toolchain, the three test layers (unit, root-only integration, and the
14-kernel VM matrix) and the benchmark harness are documented in
[CONTRIBUTING.md](CONTRIBUTING.md). Bug reports want `uname -a`, the distro and
the architecture; a CO-RE issue cannot be triaged without them.

## Security

> [!IMPORTANT]
> truetop loads eBPF with `CAP_BPF` and `CAP_PERFMON`, or root. Report
> vulnerabilities privately through the [security policy](SECURITY.md), not a
> public issue.

## License

User space is MIT or Apache-2.0. The eBPF code in `truetop-ebpf/` is GPL-2.0 or
MIT, matching the licence it declares to the kernel verifier.
