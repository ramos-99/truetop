//! switch: monitoring cost under a high context-switch rate with few processes.
//! Needs root.

use anyhow::Result;

use super::sudo_script;

pub(super) fn run() -> Result<()> {
    eprintln!("== switch ==");
    sudo_script("bench/switch/run.sh")
}
