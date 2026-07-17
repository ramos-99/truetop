#!/usr/bin/env bash
#
# self-cpu benchmark: each monitor's own CPU% and RSS under a controlled process
# load, sampled as a distribution (median + IQR). User-space cost only; truetop's
# total also carries the kernel sched_switch cost (see hotpath and switch). truetop
# appears twice: with its UI (parity with btop/htop, which render) and as the
# headless collector (--bench), so the renderer's share is visible.
# See ../BENCHMARKS.md.
#
# Quiet machine, on AC (xtask tunes governor/turbo): sudo ./run.sh
set -euo pipefail
cd "$(dirname "$0")"
source ../lib.sh
export TERM="${TERM:-xterm-256color}"

# Windows span several refresh periods so each is a stable average: these are
# periodic bursty workloads, so sub-refresh windows would just be bimodal.
COUNTS=(0 1000 2500 5000 10000)
WINDOWS=6
INTERVAL=3
WARMUP=1
ROWS=50
COLS=200
LOAD=../../target/release/load
TRUETOP=../../target/release/truetop
RESULTS="$(cd "$(dirname "$0")/.." && pwd)/results"
OUT="$RESULTS/selfcpu.csv"
mkdir -p "$RESULTS"
TCK=$(getconf CLK_TCK)
PAGE=$(getconf PAGESIZE)

require script btop htop pgrep getconf
require_built "$LOAD" "$TRUETOP"
require_root
warn_if_not_performance selfcpu

BTOP_HOME=$(btop_home)
load_pid=
wrapper=
pid=
cleanup() {
    [[ -n $pid ]] && kill "$pid" 2>/dev/null || true
    [[ -n $wrapper ]] && kill "$wrapper" 2>/dev/null || true
    [[ -n $load_pid ]] && kill "$load_pid" 2>/dev/null || true
    rm -rf "$BTOP_HOME"
}
trap cleanup EXIT

rss_mb() {
    local pages
    pages=$(awk '{print $2}' "/proc/$1/statm" 2>/dev/null || echo 0)
    awk "BEGIN { printf \"%.1f\", $pages * $PAGE / 1048576 }"
}

# measure LABEL PGREP_NAME MODE CMD...
#   MODE pty runs the TUI under a pseudo-terminal at fixed geometry; bare runs a
#   headless process directly.
measure() {
    local label=$1 name=$2 mode=$3
    shift 3
    if [[ $mode == pty ]]; then
        script -qfc "stty rows $ROWS cols $COLS 2>/dev/null; exec $*" /dev/null >/dev/null 2>&1 &
    else
        "$@" >/dev/null 2>&1 &
    fi
    wrapper=$!
    sleep 1
    pid=$(pgrep -n -x "$name" 2>/dev/null || true)
    if [[ -z $pid ]]; then
        echo "  $label: not found" >&2
        kill "$wrapper" 2>/dev/null || true
        wrapper=
        return
    fi

    local samples="" c0 t0 c1 t1 dc
    c0=$(cpu_ticks "$pid")
    t0=$(date +%s.%N)
    for _ in $(seq 1 "$WINDOWS"); do
        sleep "$INTERVAL"
        kill -0 "$pid" 2>/dev/null || break
        c1=$(cpu_ticks "$pid")
        t1=$(date +%s.%N)
        dc=$((c1 - c0))
        ((dc >= 0)) && samples+=$(awk "BEGIN { printf \"%.2f\", ($dc / $TCK) / ($t1 - $t0) * 100 }")$'\n'
        c0=$c1
        t0=$t1
    done
    local rss
    rss=$(rss_mb "$pid")

    kill "$pid" "$wrapper" 2>/dev/null || true
    pid=
    wrapper=

    local med p25 p75
    read -r _ med p25 p75 _ _ <<<"$(printf '%s' "$samples" | iqr_stats)"
    printf 'scale,%s,%d,%s,%s,%s,%s\n' "$label" "$PROCS" "$med" "$p25" "$p75" "$rss" >>"$OUT"
}

echo "scenario,tool,procs,cpu_median,cpu_p25,cpu_p75,rss_mb" >"$OUT"
for n in "${COUNTS[@]}"; do
    if ((n > 0)); then
        "$LOAD" "$n" &
        load_pid=$!
        sleep 0.5
    fi
    PROCS=$(find /proc -maxdepth 1 -regex '/proc/[0-9]+' | wc -l)
    echo "== $PROCS processes ==" >&2

    measure htop htop pty htop -d 10
    measure btop btop pty env "XDG_CONFIG_HOME=$BTOP_HOME" btop
    measure truetop-ui truetop pty "$TRUETOP"
    measure truetop-collector truetop bare "$TRUETOP" --bench 100000

    if [[ -n $load_pid ]]; then
        kill "$load_pid" 2>/dev/null || true
        wait "$load_pid" 2>/dev/null || true
        load_pid=
    fi
done

echo "wrote $OUT" >&2
