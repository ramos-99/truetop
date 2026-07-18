//! Fail-fast: a real unprivileged eBPF load must surface EPERM as the
//! `PermissionDenied` the hint classifier keys on. The unit tests prove the
//! classifier maps `PermissionDenied` to a hint; only a live kernel proves aya
//! actually produces one. Privilege is dropped in-process (no exec, so no
//! artifact-path issues) and this is its own test binary, so shedding it here is
//! contained. Run: `cargo xtask test`.

use std::io::ErrorKind;

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
