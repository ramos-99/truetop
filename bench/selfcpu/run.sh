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
CHURN=../../target/release/churn
TRUETOP=../../target/release/truetop
RESULTS="$(cd "$(dirname "$0")/.." && pwd)/results"
OUT="$RESULTS/selfcpu.csv"
mkdir -p "$RESULTS"
TCK=$(getconf CLK_TCK)
PAGE=$(getconf PAGESIZE)

require script btop htop pgrep getconf
require_built "$LOAD" "$CHURN" "$TRUETOP"
require_root
warn_if_not_performance selfcpu

BTOP_HOME=$(btop_home)
load_pid=
churn_pid=
wrapper=
pid=
cleanup() {
    [[ -n $pid ]] && kill "$pid" 2>/dev/null || true
    [[ -n $wrapper ]] && kill "$wrapper" 2>/dev/null || true
    [[ -n $load_pid ]] && kill "$load_pid" 2>/dev/null || true
    [[ -n $churn_pid ]] && kill "$churn_pid" 2>/dev/null || true
    rm -rf "$BTOP_HOME"
}
trap cleanup EXIT

rss_mb() {
    local pages
    pages=$(awk '{print $2}' "/proc/$1/statm" 2>/dev/null || echo 0)
    awk "BEGIN { printf \"%.1f\", $pages * $PAGE / 1048576 }"
}

# Count live processes from directory names alone; `find` stats each entry and
# fails when one vanishes mid-scan, which is constant under churn.
count_procs() {
    ls -1 /proc | grep -cE '^[0-9]+$'
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
    printf '%s,%s,%d,%s,%s,%s,%s\n' "$SCENARIO" "$label" "$PROCS" "$med" "$p25" "$p75" "$rss" >>"$OUT"
}

echo "scenario,tool,procs,cpu_median,cpu_p25,cpu_p75,rss_mb" >"$OUT"

SCENARIO=scale
for n in "${COUNTS[@]}"; do
    if ((n > 0)); then
        "$LOAD" "$n" &
        load_pid=$!
        sleep 0.5
    fi
    PROCS=$(count_procs)
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

# Churn: high process turnover at a low live count - the exit rate a parallel
# build produces, which drives truetop's per-exit reaping that the count scan
# above never touches. htop/btop are refresh-bound and barely feel it; this is
# where truetop pays, so measure it.
# Drain the last scale load first: its processes are still being reaped, and
# would otherwise inflate churn's live count. Wait until the count stops falling.
settle=$(count_procs)
sleep 2
while (($(count_procs) + 20 < settle)); do
    settle=$(count_procs)
    sleep 2
done

SCENARIO=churn
"$CHURN" &
churn_pid=$!
sleep 0.5
p0=$(awk '/^processes /{print $2}' /proc/stat)
sleep 3
p1=$(awk '/^processes /{print $2}' /proc/stat)
RATE=$(((p1 - p0) / 3))
PROCS=$(count_procs)
echo "== churn: ~$RATE forks/sec at $PROCS live processes ==" >&2
echo "forks_per_sec=$RATE live_processes=$PROCS" >"$RESULTS/churn.txt"

measure htop htop pty htop -d 10
measure btop btop pty env "XDG_CONFIG_HOME=$BTOP_HOME" btop
measure truetop-ui truetop pty "$TRUETOP"
measure truetop-collector truetop bare "$TRUETOP" --bench 100000

kill "$churn_pid" 2>/dev/null || true
wait "$churn_pid" 2>/dev/null || true
churn_pid=

echo "wrote $OUT" >&2
