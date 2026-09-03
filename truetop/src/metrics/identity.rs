//! Process identity: name and owning user, both captured in the kernel on `exec`
//! and `fork` into `COMM_MAP`. A one-time `/proc` snapshot seeds processes that
//! predate truetop; uids are mapped through `/etc/passwd`, read once.

use std::collections::HashMap;

use aya::maps::{HashMap as BpfHashMap, MapData};
use truetop_common::{COMM_LEN, Identity};

const UNKNOWN: &str = "<unknown>";

/// Identity of a process that predates truetop, read once from `/proc`.
struct Seeded {
    name: String,
    uid: u32,
}

/// A process's `comm` and the passwd name for its `cred->uid`.
pub(crate) struct Named {
    pub(crate) name: String,
    pub(crate) user: Option<String>,
}

pub(crate) struct Resolver {
    comm: BpfHashMap<MapData, u32, Identity>,
    seed: HashMap<u32, Seeded>,
    users: HashMap<u32, String>,
}

impl Resolver {
    pub(crate) fn new(comm: BpfHashMap<MapData, u32, Identity>) -> Self {
        Self {
            comm,
            seed: backfill_proc_names(),
            users: load_passwd(),
        }
    }

    /// Both fields from one `COMM_MAP` read, since a single `Identity` carries
    /// them. The live map wins; fall back to the startup `/proc` snapshot, which
    /// is all a process predating truetop has.
    pub(crate) fn named(&self, tgid: u32) -> Named {
        let (name, uid) = match self.comm.get(&tgid, 0) {
            Ok(identity) => (decode_comm(identity.comm), Some(identity.uid)),
            Err(_) => match self.seed.get(&tgid) {
                Some(seeded) => (seeded.name.clone(), Some(seeded.uid)),
                None => (UNKNOWN.to_owned(), None),
            },
        };
        Named {
            name,
            user: uid.map(|uid| self.passwd_name(uid)),
        }
    }

    /// Drop a departed process's name once user space has finished with it, so
    /// `COMM_MAP` does not grow without bound (the exit path no longer clears it
    /// in the kernel; see `reaper`).
    pub(crate) fn forget(&mut self, tgid: u32) {
        let _ = self.comm.remove(&tgid);
        self.seed.remove(&tgid);
    }

    /// The `/etc/passwd` name for `uid`, or the number when it maps to no entry.
    fn passwd_name(&self, uid: u32) -> String {
        self.users
            .get(&uid)
            .cloned()
            .unwrap_or_else(|| uid.to_string())
    }
}

fn decode_comm(raw: [u8; COMM_LEN]) -> String {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).trim().to_owned()
}

/// Real uid from `/proc/<pid>/status` (the first of the four `Uid:` fields).
/// Startup only: live processes carry their uid in `COMM_MAP`.
fn proc_uid(pid: u32) -> Option<u32> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let line = status.lines().find(|l| l.starts_with("Uid:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

fn load_passwd() -> HashMap<u32, String> {
    let content = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    content
        .lines()
        .filter_map(|line| {
            // name:passwd:uid:gid:...
            let mut fields = line.split(':');
            let name = fields.next()?;
            let uid = fields.nth(1)?.parse().ok()?;
            Some((uid, name.to_owned()))
        })
        .collect()
}

/// Seed identity for processes that predate truetop and never fired a capturable
/// `exec`. The `status` read is the expensive one, and it happens once per
/// process here rather than once per row per tick.
fn backfill_proc_names() -> HashMap<u32, Seeded> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return HashMap::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let pid = e.file_name().to_str()?.parse().ok()?;
            let name = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
            Some((
                pid,
                Seeded {
                    name: name.trim().to_owned(),
                    uid: proc_uid(pid)?,
                },
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comm(bytes: &[u8]) -> [u8; COMM_LEN] {
        let mut raw = [0u8; COMM_LEN];
        raw[..bytes.len()].copy_from_slice(bytes);
        raw
    }

    #[test]
    fn decode_comm_stops_at_nul() {
        assert_eq!(decode_comm(comm(b"bash\0ignored")), "bash");
    }

    #[test]
    fn decode_comm_reads_full_buffer() {
        assert_eq!(decode_comm([b'a'; COMM_LEN]), "a".repeat(COMM_LEN));
    }

    #[test]
    fn decode_comm_is_lossy_on_invalid_utf8() {
        assert_eq!(decode_comm(comm(&[0xff, 0xff])), "\u{fffd}\u{fffd}");
    }
}
