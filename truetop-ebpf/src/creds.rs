//! Credential changes. `exec` and `fork` capture the uid a process is born with,
//! but one that calls `setuid` afterwards - an nginx worker, an sshd session -
//! would keep the old one forever. `commit_creds` is the single point every
//! credential change passes through, so one hook covers all of them.
//!
//! Not a `raw_tracepoint` like the rest: the kernel exposes no tracepoint for
//! this. `fentry` costs a trampoline on a path that fires about once per process
//! lifetime, not per event (CLAUDE.md §2).

use aya_ebpf::{
    Global,
    helpers::{bpf_get_current_pid_tgid, bpf_probe_read_kernel},
    macros::fentry,
    programs::FEntryContext,
};

use crate::comm;

// Byte offset of `cred::uid`, resolved from kernel BTF and injected at load.
#[unsafe(no_mangle)]
static CRED_UID_OFFSET: Global<u32> = Global::new(0);

#[fentry(function = "commit_creds")]
pub fn commit_creds(ctx: FEntryContext) -> i32 {
    // args: (*new). These credentials are not installed yet, so the new uid comes
    // from the argument; `bpf_get_current_uid_gid` would still return the old one.
    let new: *const u8 = ctx.arg(0);
    if new.is_null() {
        return 0;
    }
    let field = unsafe { new.add(CRED_UID_OFFSET.load() as usize) } as *const u32;
    let Ok(uid) = (unsafe { bpf_probe_read_kernel(field) }) else {
        return 0;
    };

    let tgid = (bpf_get_current_pid_tgid() >> 32) as u32;
    if tgid != 0 {
        comm::set_uid(tgid, uid);
    }
    0
}
