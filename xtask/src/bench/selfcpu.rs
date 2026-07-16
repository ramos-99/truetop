//! self-cpu: each monitor's own CPU% and RSS under load, vs btop/htop. Needs root.

use std::process::Command;

use anyhow::Result;

use super::{exec, root, sudo_script};

pub(super) fn run() -> Result<()> {
    eprintln!("== self-cpu ==");
    sudo_script("bench/selfcpu/run.sh")?;
    if exec(&mut Command::new(root().join("bench/selfcpu/plot.py"))).is_err() {
        eprintln!("self-cpu: plot skipped (needs python + matplotlib)");
    }
    Ok(())
}
