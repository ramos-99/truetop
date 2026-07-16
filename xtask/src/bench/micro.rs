//! micro: criterion model of the per-process collection cost, no root.

use std::process::Command;

use anyhow::{Context as _, Result};

use super::{exec, have, results_dir};
use crate::runner::cargo;

pub(super) fn run() -> Result<()> {
    eprintln!("== micro ==");
    let criterion = results_dir().join("criterion");
    let _ = std::fs::remove_dir_all(&criterion);

    // Pin to one core so the estimates don't ride scheduler migration.
    let mut cmd = if have("taskset") {
        let mut c = Command::new("taskset");
        c.args(["-c", "1", &cargo()]);
        c
    } else {
        Command::new(cargo())
    };
    cmd.args(["bench", "-p", "truetop-bench"])
        .env("CRITERION_HOME", &criterion);
    exec(&mut cmd).context("micro benchmark")
}
