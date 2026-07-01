# Benchmarks

![collection](https://img.shields.io/badge/collection-eBPF-F76800?style=flat-square&logo=linux&logoColor=white)
![syscalls](https://img.shields.io/badge/syscalls-O%281%29-2EA043?style=flat-square)
![vs procfs](https://img.shields.io/badge/vs_procfs-O%28N%29-C74634?style=flat-square)

![harness](https://img.shields.io/badge/harness-criterion_%C2%B7_strace_%C2%B7_bpftool-8250DF?style=flat-square)
![built with](https://img.shields.io/badge/Rust-2024-DEA584?style=flat-square&logo=rust&logoColor=white)

Per-process CPU is collected in-kernel and pulled in one batched syscall, instead
of parsing `/proc/<pid>/stat` once per process. That makes CPU collection O(1) in
syscalls per refresh instead of O(N). The price is an O(1) eBPF program on every
context switch, measured in the hotpath benchmark below.

## Result

![CPU collection cost versus process count](results/scaling.svg)

Per-process data syscalls per refresh, traced with `strace` against the real
tools:

| Processes | top    | htop   | truetop | truetop's edge |
| --------: | -----: | -----: | ------: | -------------: |
|       362 |    728 |    332 | **309** |           1.1x |
|       613 |  1,230 |    896 | **783** |           1.1x |
|       863 |  1,731 |  1,481 | **782** |           1.9x |
|     1,364 |  2,733 |  2,374 | **782** |           3.0x |
|     2,364 |  4,733 |  5,127 | **782** |           6.0x |
|     5,365 | 10,449 | 11,959 | **782** |          13.4x |

top and htop read one `/proc/<pid>` file per process, so their cost grows with
process count. truetop batch-reads the CPU map once and touches `/proc` only for
the visible viewport, so it stays flat. At 5,365 processes it issues ~780 syscalls
per refresh against htop's ~11,959. Edge is the fewest-syscall competitor divided
by truetop; lower is better.

## Running

```sh
cargo xtask bench                  # all three
cargo xtask bench micro macro      # or any subset, in any order
```

One command builds what it needs, runs the benchmarks, and for stable numbers
pins the CPU governor to `performance`, disables turbo, and pins the micro
benchmark to one core. It restores the original CPU settings on exit, including on
error, so the machine is never left tuned. Pass `--no-prep` to skip the tuning (on
a VM or in CI). macro and hotpath need root, so it elevates with `sudo`.

Every benchmark writes to `bench/results/`: the criterion report under
`criterion/`, plus `macro.csv`, `scaling.svg`, and `hotpath.txt`. Only
`scaling.svg` is tracked in git; the rest is regenerated per run.

### Requirements

micro needs only a Rust toolchain; macro and hotpath shell out to standard tools
and need root. Each script checks its own dependencies and names any that are
missing before doing work, so a partial install fails fast rather than mid-run.

| benchmark | tools it calls                                                            |
| --------- | ------------------------------------------------------------------------- |
| micro     | `cargo` (criterion is a dev-dependency); `taskset` used for pinning if present |
| macro     | `strace`, `script`, `top`, `htop`; `python3` + `matplotlib` for the plot  |
| hotpath   | `bpftool`, `jq`, `hackbench`                                               |

`script`, `taskset`, and `timeout` come from util-linux/coreutils and are almost
always already installed. The rest, by distro:

```sh
# Arch (hackbench is in the AUR via rt-tests)
sudo pacman -S --needed strace htop bpf jq python-matplotlib

# Debian / Ubuntu
sudo apt install strace htop jq rt-tests python3-matplotlib linux-tools-$(uname -r)
```

Package names vary; the table above is the source of truth for what must be on
`PATH`. `bpftool` ships with the kernel tools (`bpf` on Arch,
`linux-tools-$(uname -r)` on Debian), and `hackbench` ships with `rt-tests`.

## At a glance

|             | Question it answers                | Method                            | Run                          |
| ----------- | ---------------------------------- | --------------------------------- | ---------------------------- |
| **micro**   | Is the per-process *work* cheaper? | in-memory model, no `bpf()` call  | `cargo xtask bench micro`    |
| **macro**   | Does it cut *syscalls* at scale?   | `strace` vs top / htop            | `cargo xtask bench macro`    |
| **hotpath** | What does the kernel work *cost*?  | `bpftool` run stats + `hackbench` | `cargo xtask bench hotpath`  |

The three measure different things. The micro is the per-process cost, the macro
is how that adds up across processes, and the hotpath is the kernel-side cost the
other two ignore.

## micro: per-process work

```sh
cargo xtask bench micro            # wraps: cargo bench -p truetop-bench
```

Sweeps N and compares the two collection paths. Both compute the same thing, the
total on-CPU nanoseconds over N processes:

| path             | source                                         | syscalls |
| ---------------- | ---------------------------------------------- | -------- |
| `ebpf_batched`   | fold per-CPU slots into the `tgid → ns` map    | O(1)     |
| `procfs_per_pid` | open / read / parse `/proc/<pid>/stat` per pid | O(N)     |

The eBPF arm builds a `HashMap` the same way `batch::fold` does: it sums each
key's per-CPU slots and inserts the result. A bare summation would leave out the
insert and undercount what the collector actually does per process.

Criterion writes a log-Y HTML report to `bench/results/criterion/`. Both lines
rise linearly, and the eBPF one sits orders of magnitude lower. On the reference
machine (AMD Ryzen 7 5800HS, 16 cores) that is roughly 30 ns per process for eBPF
against 12 µs for procfs, about 400x. The figures are machine-specific, so
regenerate them for yours; the shape holds.

`procfs_per_pid` re-reads `/proc/self/stat`, one cached file, which is the best
case for procfs. The macro benchmark reads N distinct files, so the real gap is
wider. What the collector actually issues per tick (`batch::BatchReader`) is
measured in the macro benchmark below.

`cargo xtask bench micro` wipes `bench/results/criterion` and runs under the performance
governor, so each run starts from a clean baseline. Running `cargo bench` directly
skips that, and a busy machine then prints spurious regressed and improved lines
against a stale baseline.

## macro: versus top and htop

The chart and table at the top of this doc come from here.

```sh
cargo xtask bench macro            # wraps: sudo bench/macro/run.sh + plot.py
```

Counts per-process data syscalls per refresh against the real tools under
`strace -fy`: `read` / `pread64` on `/proc/<pid>/{stat,statm,status,cmdline}`
(`-y` resolves cached fds back to paths, so reads off a held fd still count) plus
`bpf`, divided by refreshes. A load generator (`src/bin/load.rs`) forks N
processes that reschedule at ~0% CPU. Truly idle ones never reschedule, so no
monitor would see them anyway.

These per-refresh figures are an approximation, not an exact count. Reads are
divided by a refresh count inferred per tool: top and htop by each reopen of the
`/proc` directory, truetop by its fixed tick count. The proxy is close, because
top and htop genuinely reopen `/proc` every scan, but read the numbers as the
scaling shape rather than exact syscall counts.

The collector reads the CPU map with `BPF_MAP_LOOKUP_BATCH` (one call per 4096
live pids, so one in the common case) and touches `/proc` only for the visible
viewport: 256 rows, each a `statm` read plus a `comm` map lookup, which is itself
a `bpf` syscall. So the per-refresh cost is O(N/4096) batch calls plus O(viewport)
lookups, flat in N once the viewport saturates. The flat ~782 is the saturated
256-row viewport; at 362 processes (mostly idle) truetop tracks the active count
instead. The visible crossing in the plot is between top and htop, not truetop.

There is a crossover, but only below the point where it matters. If fewer than
about 256 processes are all active at once, procfs's single stat per file can beat
truetop's per-row read plus name lookup. Real systems run hundreds of processes
with few active, so truetop wins in practice, and the gap widens without bound as
N grows.

**Why `--bench`.** truetop runs headless (`truetop --bench <ticks>`). `strace`
ptrace-traps every syscall, and the TUI input poll issues enough of them to starve
the collector under tracing, so the traced run never completes a tick. `--bench`
drives the collector directly with no terminal, so the trace covers the collection
path only. The tick count is fixed, so reads divide by it without a per-scan
marker.

**Why btop is omitted.** It reads `/proc/<pid>` per process like htop (same O(N)
class), but its TUI defeats `strace` the same way truetop's does. htop covers the
procfs case.

Not counted on truetop's side: the `sched_switch` program, which runs in the
kernel on every context switch. The count here is the user-space refresh cost,
where the O(1) vs O(N) split is. The kernel cost is the hotpath benchmark.

## hotpath: kernel cost per context switch

```sh
cargo xtask bench hotpath          # wraps: sudo bench/hotpath/run.sh
```

The macro benchmark counts the per-process syscalls truetop avoids in user space.
This one counts what it adds in the kernel. `sched_switch` runs on every context
switch, which is the trade-off the README's Overhead section describes.

With `kernel.bpf_stats_enabled` set, `bpftool prog show` reports `run_cnt` and
`run_time_ns` per program, and their ratio is the per-event cost. The script
samples that across a `hackbench` run and also times `hackbench` with and without
truetop attached, so the per-event figure has a wall-clock number behind it.

| metric       | source                                              |
| ------------ | --------------------------------------------------- |
| per-event ns | `run_time_ns / run_cnt` for `sched_switch`          |
| overhead %   | `hackbench` wall-clock, without versus with truetop |

On the reference machine (AMD Ryzen 7 5800HS, 16 cores), under `hackbench`,
truetop adds **+8.3%** wall-clock (4.41s to 4.77s) at **~1.6 µs per context
switch** over 12M events. The two numbers agree: 1.6 µs times 12M events is ~20
CPU-seconds of program time, which over the ~14s window across 16 cores is ~8.5%,
matching the wall-clock delta. So the per-event figure is a real cost, not a
`bpf_stats` artefact, and it includes the cache-cold `task_struct` reads a
context-switch storm forces.

This is a worst case. hackbench drives roughly 800k context switches per second
across the machine; truetop's overhead scales with that rate, not with process
count, so a normal system doing orders of magnitude fewer switches pays
proportionally less, well under 1%. Enabling run statistics adds a small per-run
cost of its own, so the figure is if anything slightly pessimistic. Re-run on your
hardware; the shape holds.
