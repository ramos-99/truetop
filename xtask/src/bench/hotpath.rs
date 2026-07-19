//! hotpath: per-event kernel cost of `sched_switch` under hackbench. Needs root.

use anyhow::Result;

use super::root_script;

pub(super) fn run() -> Result<()> {
    eprintln!("== hotpath ==");
    root_script("bench/hotpath/run.sh")
}
