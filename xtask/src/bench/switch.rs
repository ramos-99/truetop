//! switch: monitoring cost under a high context-switch rate with few processes.
//! Needs root.

use anyhow::Result;

use super::root_script;

pub(super) fn run() -> Result<()> {
    eprintln!("== switch ==");
    root_script("bench/switch/run.sh")
}
