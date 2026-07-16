//! macro: per-process syscalls per refresh vs top/htop, via strace. Needs root.

use std::process::Command;

use anyhow::Result;

use super::{exec, root, sudo_script};

pub(super) fn run() -> Result<()> {
    eprintln!("== macro ==");
    sudo_script("bench/macro/run.sh")?;
    if exec(&mut Command::new(root().join("bench/macro/plot.py"))).is_err() {
        eprintln!("macro: plot skipped (needs python + matplotlib)");
    }
    Ok(())
}
