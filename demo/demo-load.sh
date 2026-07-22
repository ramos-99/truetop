#!/usr/bin/env bash
# Stages a real workload for the demo: synchronous random O_DIRECT reads that
# genuinely block on the device (uninterruptible D-state, which truetop charges
# as I/O wait) plus one CPU-bound process for contrast. Nothing is faked - the
# reads really hit the disk.
#
# Random 4k reads are latency-bound: each read waits a full device round-trip, so
# the reader sits at ~100% I/O wait even on fast NVMe. Bandwidth-bound big
# sequential reads do not - the device just streams them and nothing blocks.
#
#   ./demo/demo-load.sh start   # spawn the workload
#   ./demo/demo-load.sh stop    # kill it and clean up
set -euo pipefail

# O_DIRECT needs a real block-backed filesystem, so keep the scratch on the repo
# disk, never /tmp (commonly tmpfs).
SCRATCH="./truetop-demo.scratch"
PIDFILE="./truetop-demo.pids"
SIZE_MB=512
READERS=4

start() {
    command -v fio >/dev/null || {
        echo "need fio for a latency-bound disk load: sudo pacman -S fio" >&2
        exit 1
    }
    dd if=/dev/urandom of="$SCRATCH" bs=1M count="$SIZE_MB" status=none
    : >"$PIDFILE"

    # CPU-bound: pegs a core, ~0 I/O wait.
    yes >/dev/null &
    echo $! >>"$PIDFILE"

    # Disk-bound: synchronous random O_DIRECT reads, ~100% I/O wait with ~0 CPU.
    # --thread + one process per reader so each is an exec'd `fio` truetop can name;
    # forked job workers never exec and would show as <unknown>.
    for _ in $(seq "$READERS"); do
        fio --name=diskstall --filename="$SCRATCH" \
            --rw=randread --bs=4k --direct=1 --ioengine=sync --thread \
            --numjobs=1 --time_based --runtime=3600 >/dev/null 2>&1 &
        echo $! >>"$PIDFILE"
    done

    echo "demo load: 1 cpu hog + $READERS disk readers (raise READERS if I/O wait looks low)"
}

stop() {
    if [ -f "$PIDFILE" ]; then
        while read -r pid; do kill "$pid" 2>/dev/null || true; done <"$PIDFILE"
    fi
    # Reap any fio worker still reading the scratch file.
    pkill -f "$SCRATCH" 2>/dev/null || true
    rm -f "$SCRATCH" "$PIDFILE"
    echo "demo load: stopped"
}

case "${1:-}" in
start) start ;;
stop) stop ;;
*)
    echo "usage: $0 {start|stop}" >&2
    exit 1
    ;;
esac
