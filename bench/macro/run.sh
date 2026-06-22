#!/usr/bin/env bash
#
# Macro benchmark: per-process data syscalls per refresh vs process count.
# top/htop fetch one /proc/<pid> file per process per refresh (O(N)); truetop
# batch-reads the CPU map (O(1)) and only touches /proc for the visible viewport.
# truetop runs headless (--bench) so strace sees only the collector, not the TUI.
# See ../BENCHMARKS.md. Quiet machine, on AC: sudo ./run.sh && ./plot.py
set -euo pipefail
cd "$(dirname "$0")"

COUNTS=(0 250 500 1000 2000 5000)
DURATION=20
TICKS=15            # truetop headless collector ticks
LOAD=../../target/release/load
TRUETOP=../../target/release/truetop
OUT=results.csv

for c in strace script top htop; do
    command -v "$c" >/dev/null || { echo "missing dependency: $c" >&2; exit 1; }
done
for b in "$LOAD" "$TRUETOP"; do
    [[ -x $b ]] || { echo "build first: cargo build --release" >&2; exit 1; }
done
[[ $EUID -eq 0 ]] || { echo "run as root (truetop loads eBPF): sudo $0" >&2; exit 1; }

echo "tool,procs,syscalls_per_refresh" > "$OUT"
load_pid=
trap 'kill "$load_pid" 2>/dev/null || true' EXIT

# strace a tool for DURATION s; timeout wraps the tool (inside strace) so it
# exits cleanly and strace flushes its log.
record() {
    local log=$1; shift
    script -qfc \
        "strace -fy -e trace=openat,openat2,read,pread64,bpf -o '$log' timeout -s TERM $DURATION $* >/dev/null 2>&1" \
        /dev/null >/dev/null 2>&1 || true
}

# Per-process data reads (-y resolves fds to /proc paths, so cached fds count
# too) plus bpf, divided by refreshes (procfs tools reopen /proc per scan;
# truetop issues one batch per tick) — refresh-rate independent.
per_refresh() {
    local log=$1 refresh_re=$2 reads refreshes
    reads=$(grep -cE '(read|pread64)\([0-9]+</proc/[0-9]+/(stat|statm|status|cmdline)|\bbpf\(' \
        "$log" 2>/dev/null || true)
    refreshes=$(grep -cE "$refresh_re" "$log" 2>/dev/null || true)
    ((refreshes > 0)) || refreshes=1
    echo $((reads / refreshes))
}

for n in "${COUNTS[@]}"; do
    "$LOAD" "$n" &
    load_pid=$!
    sleep 0.5
    procs=$(find /proc -maxdepth 1 -regex '/proc/[0-9]+' | wc -l)
    echo "== $procs processes ==" >&2

    log=$(mktemp)
    record "$log" top -b && echo "top,$procs,$(per_refresh "$log" '"/proc", O_')" >> "$OUT"
    record "$log" htop && echo "htop,$procs,$(per_refresh "$log" '"/proc", O_')" >> "$OUT"

    # truetop is headless here, so the tick count is known — divide by it
    # directly instead of grepping a per-scan marker.
    record "$log" "$TRUETOP" --bench "$TICKS"
    reads=$(grep -cE '(read|pread64)\([0-9]+</proc/[0-9]+/(stat|statm|status|cmdline)|\bbpf\(' \
        "$log" 2>/dev/null || true)
    echo "truetop,$procs,$((reads / TICKS))" >> "$OUT"
    rm -f "$log"

    kill "$load_pid" 2>/dev/null || true
    wait "$load_pid" 2>/dev/null || true
    load_pid=
done

echo "wrote $OUT" >&2
