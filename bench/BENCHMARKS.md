# Benchmarks

Extracting per-process CPU is O(1) in syscalls with eBPF vs O(N) with procfs.
Both are O(N) in time; the difference is a constant factor — the eBPF path makes
no per-process syscall, avoiding the VFS open/read and the text formatting of
`/proc/<pid>/stat`.

## scaling

```sh
cargo bench -p truetop-bench
```

Sweeps N and compares two ways to get the same result (total on-CPU ns over N
processes):

| path           | source                                       | syscalls |
| -------------- | -------------------------------------------- | -------- |
| ebpf_batched   | one `bpf_map_lookup_batch` (modelled in mem) | O(1)     |
| procfs_per_pid | open/read/parse `/proc/<pid>/stat` per pid   | O(N)     |

Criterion writes a log-Y HTML report to `target/criterion/`. Both lines rise
linearly; the eBPF one sits ~3500x lower (~4 ns vs ~14 µs per process).

`ebpf_batched` is modelled in memory, so it makes no real `bpf()` call — it
measures the per-process work, not the syscall count. The syscall count is the
collector itself (`batch::BatchReader`, one `BPF_MAP_LOOKUP_BATCH` per tick),
verifiable with `strace -c` in the macro benchmark.

`procfs_per_pid` re-reads `/proc/self/stat` (one cached file), the best case for
procfs; btop reads N distinct files, so the real gap is wider.

Not measured here: the `sched_switch` program runs on every context switch (O(1)
per event, not zero); `bpftool prog show` reports that cost.

Results compare against the last run's baseline, so a busy machine prints
spurious regressed/improved lines — `rm -rf target/criterion` and pin the
governor to `performance` for clean numbers.

## macro: vs top and htop

```sh
sudo ./macro/run.sh && ./macro/plot.py   # writes scaling.svg
```

Counts per-process data syscalls per refresh against the real tools under
`strace -fy`: `read`/`pread64` on `/proc/<pid>/{stat,statm,status,cmdline}`
(`-y` resolves cached fds back to paths, so reads off a held fd still count)
plus `bpf`, divided by refreshes. A load generator (`src/bin/load.rs`) forks N
idle children to sweep the process count.

truetop runs headless (`truetop --bench <ticks>`). strace ptrace-traps every
syscall, and the TUI input poll issues enough of them to starve the collector
under tracing — the traced run never completes a tick. `--bench` drives the
collector directly with no terminal, so the trace is the collection path only;
the tick count is fixed, so reads divide by it without a per-scan marker.

btop is omitted. It reads `/proc/<pid>` per process like htop (same O(N) class)
but its TUI defeats strace the same way truetop's does; htop covers the procfs
case.

![scaling](macro/scaling.svg)

top and htop are linear: one `/proc/<pid>` file per process per refresh. truetop
is flat — the collector reads the CPU map in one batch and touches `/proc` only
for the visible viewport (256 rows, one `statm` read and one name lookup each),
independent of N.

The lines cross at a few hundred processes: below that the procfs tools open
fewer files than the viewport and are as cheap or cheaper. truetop wins past it,
and the gap is unbounded — flat vs linear, ~12-15x fewer syscalls at 5300
processes and diverging. The win is asymptotic, not at every scale.

Not counted on truetop's side: the `sched_switch` program, which runs in the
kernel on every context switch (`bpftool prog show` reports that cost). The
syscall metric is the user-space refresh cost, where the O(1)/O(N) split is.
