//! Process identity: name and owning user. The kernel captures `comm` on `exec`
//! into `COMM_MAP`; a one-time `/proc` snapshot seeds processes that predate
//! truetop. The user is the real uid from `/proc/<pid>/status`, mapped through
//! `/etc/passwd` (read once).

use std::collections::HashMap;

use aya::maps::{HashMap as BpfHashMap, MapData};
use truetop_common::COMM_LEN;

const UNKNOWN: &str = "<unknown>";

pub(crate) struct Resolver {
    comm: BpfHashMap<MapData, u32, [u8; COMM_LEN]>,
    seed: HashMap<u32, String>,
    users: HashMap<u32, String>,
}

impl Resolver {
    pub(crate) fn new(comm: BpfHashMap<MapData, u32, [u8; COMM_LEN]>) -> Self {
        Self {
            comm,
            seed: backfill_proc_names(),
            users: load_passwd(),
        }
    }

    /// Live `COMM_MAP` wins; fall back to the startup `/proc` snapshot.
    pub(crate) fn resolve(&self, tgid: u32) -> String {
        if let Ok(comm) = self.comm.get(&tgid, 0) {
            return decode_comm(comm);
        }
        self.seed
            .get(&tgid)
            .cloned()
            .unwrap_or_else(|| UNKNOWN.to_owned())
    }

    /// Owning user, or the numeric uid when it maps to no passwd entry. `None`
    /// if the process exited before its uid could be read.
    pub(crate) fn user(&self, tgid: u32) -> Option<String> {
        let uid = proc_uid(tgid)?;
        Some(
            self.users
                .get(&uid)
                .cloned()
                .unwrap_or_else(|| uid.to_string()),
        )
    }
}

fn decode_comm(raw: [u8; COMM_LEN]) -> String {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).trim().to_owned()
}

/// Real uid from `/proc/<pid>/status` (the first of the four `Uid:` fields).
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

/// Seed names for processes that predate truetop and never fired a capturable
/// `exec`.
fn backfill_proc_names() -> HashMap<u32, String> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return HashMap::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let pid = e.file_name().to_str()?.parse().ok()?;
            let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
            Some((pid, comm.trim().to_owned()))
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
