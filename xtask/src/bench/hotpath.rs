//! hotpath: per-event kernel cost of `sched_switch` under hackbench. Needs root.

use anyhow::Result;

use super::sudo_script;

pub(super) fn run() -> Result<()> {
    eprintln!("== hotpath ==");
    sudo_script("bench/hotpath/run.sh")
}
