#!/usr/bin/env bash
#
# self-cpu benchmark: each monitor's own CPU% and RSS under a controlled process
# load, sampled as a distribution (median + IQR). User-space leg only; truetop's
# total also includes the kernel sched_switch cost (see hotpath). truetop appears
# twice: with its UI (parity with btop/htop, which render) and headless collector
# (--bench), so the renderer's share is visible. See ../BENCHMARKS.md.
#
# Quiet machine, on AC (xtask tunes governor/turbo): sudo ./run.sh
set -euo pipefail
cd "$(dirname "$0")"
export TERM="${TERM:-xterm-256color}"          # the TUIs need it; sudo may drop it

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

for c in script btop htop pgrep getconf; do
    command -v "$c" >/dev/null || { echo "missing dependency: $c" >&2; exit 1; }
done
for b in "$LOAD" "$TRUETOP"; do
    [[ -x $b ]] || { echo "build first: cargo build --release" >&2; exit 1; }
done
[[ $EUID -eq 0 ]] || { echo "run as root (truetop loads eBPF): sudo $0" >&2; exit 1; }

gov=$(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null || true)
[[ ${gov:-} == performance ]] || echo "warning: governor '${gov:-unknown}' != performance; run via 'cargo xtask bench selfcpu'" >&2

# btop reads its refresh from the config; give it a 1s throwaway one.
BTOP_HOME=$(mktemp -d)
mkdir -p "$BTOP_HOME/btop"
printf 'update_ms = 1000\n' >"$BTOP_HOME/btop/btop.conf"

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

# utime + stime (ticks) of a pid. `stat` fields 14,15 sit at $12,$13 once the
# parenthesised comm is dropped.
cpu_ticks() {
    local rem
    rem=$(cut -d')' -f2- "/proc/$1/stat" 2>/dev/null) || { echo 0; return; }
    awk '{print $12 + $13}' <<<"$rem"
}

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

    printf '%s' "$samples" | awk -v warm="$WARMUP" -v label="$label" -v procs="$PROCS" -v rss="$rss" '
        NF { v[++raw] = $1 }
        END {
            n = 0
            for (i = warm + 1; i <= raw; i++) a[++n] = v[i]
            if (n == 0) { exit }
            for (i = 1; i <= n; i++)
                for (j = i + 1; j <= n; j++)
                    if (a[j] < a[i]) { t = a[i]; a[i] = a[j]; a[j] = t }
            med = (n % 2) ? a[(n + 1) / 2] : (a[n / 2] + a[n / 2 + 1]) / 2
            printf "scale,%s,%d,%.2f,%.2f,%.2f,%s\n", label, procs, med, a[int(n * 0.25) + 1], a[int(n * 0.75) + 1], rss
        }' >>"$OUT"
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
