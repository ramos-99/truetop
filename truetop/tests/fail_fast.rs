//! Fail-fast: the two failures a first run actually hits must reach the user as
//! an actionable message, not a bare error. These need a live kernel to prove
//! what unit tests cannot: that a real load without privilege surfaces EPERM as
//! `PermissionDenied`, and that a real run with the BTF gone prints the config
//! remediation. Run: `cargo xtask test`.

use std::{io::ErrorKind, process::Command};

use truetop::attach;

const NOBODY: u32 = 65534;

#[test]
#[ignore = "needs root + a live kernel; run: cargo xtask test"]
fn unprivileged_load_surfaces_a_permission_error() {
    // SAFETY: plain syscalls with no borrowed state. From root they drop us to an
    // unprivileged uid, which clears CAP_BPF; gid before uid, since it is uid 0
    // that permits the setgid.
    assert_eq!(unsafe { libc::setgid(NOBODY) }, 0, "setgid failed");
    assert_eq!(unsafe { libc::setuid(NOBODY) }, 0, "setuid failed");

    let err = match attach() {
        Ok(_) => panic!("attach must fail without CAP_BPF"),
        Err(err) => err,
    };
    let is_permission = err
        .chain()
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
        .any(|io| io.kind() == ErrorKind::PermissionDenied);
    assert!(
        is_permission,
        "expected PermissionDenied in the chain:\n{err:#}"
    );
}

#[test]
#[ignore = "needs root + a live kernel; run: cargo xtask test"]
fn missing_btf_shows_the_config_hint() {
    // Hide the kernel BTF in a private mount namespace (tmpfs over its directory),
    // then run the real binary end to end. Contained: the mount never touches the
    // host and is gone when the shell exits.
    let script = format!(
        "mount -t tmpfs none /sys/kernel/btf && exec {} --bench 1",
        env!("CARGO_BIN_EXE_truetop"),
    );
    let output = Command::new("unshare")
        .args([
            "--mount",
            "--propagation",
            "private",
            "--",
            "sh",
            "-c",
            &script,
        ])
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn unshare");

    assert!(
        !output.status.success(),
        "truetop should fail with the BTF hidden"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("CONFIG_DEBUG_INFO_BTF"),
        "expected the BTF config hint in stderr, got:\n{stderr}"
    );
}
