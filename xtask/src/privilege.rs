//! One policy for running an xtask step with the privilege eBPF needs at load.
//! `cargo xtask` normally starts unprivileged and elevates with `sudo -E`; inside
//! a vmtest VM the process is already root, where sudo may be absent, so it runs
//! the program directly.

use std::process::Command;

/// A `Command` that runs `program` as root: directly when we already are,
/// otherwise under `sudo -E` so CARGO, RUST_LOG and the rest of the environment
/// carry through.
pub(crate) fn as_root(program: &str) -> Command {
    // SAFETY: geteuid takes no arguments and cannot fail.
    if unsafe { libc::geteuid() } == 0 {
        return Command::new(program);
    }
    let mut cmd = Command::new("sudo");
    cmd.arg("-E").arg(program);
    cmd
}
