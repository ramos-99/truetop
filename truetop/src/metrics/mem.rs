//! Resident set size, read from `/proc/<pid>/statm` for the visible rows only.
//! Since Linux 6.2 the exact value cannot be summed from eBPF cheaply (see
//! README), so this reads the same source `top` does, at negligible cost.

/// Resident set size in bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MemMetrics {
    pub rss_bytes: u64,
}

pub(crate) struct MemReader {
    page_size: u64,
}

impl MemReader {
    pub(crate) fn new() -> Self {
        Self {
            page_size: page_size(),
        }
    }

    /// Exact RSS from `/proc/<tgid>/statm`. `None` if the process exited between
    /// the snapshot and this read.
    pub(crate) fn for_pid(&self, tgid: u32) -> Option<MemMetrics> {
        let statm = std::fs::read_to_string(format!("/proc/{tgid}/statm")).ok()?;
        Some(MemMetrics {
            rss_bytes: parse_statm_pages(&statm)? * self.page_size,
        })
    }
}

/// Resident-set pages from a `/proc/<pid>/statm` line — the second whitespace
/// field. `None` if the line is empty or malformed.
fn parse_statm_pages(statm: &str) -> Option<u64> {
    statm.split_whitespace().nth(1)?.parse().ok()
}

fn page_size() -> u64 {
    // SAFETY: sysconf with a valid name is always safe to call.
    let n = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if n > 0 { n as u64 } else { 4096 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_statm_reads_resident_pages() {
        assert_eq!(parse_statm_pages("4096 512 64 1 0 128 0"), Some(512));
        assert_eq!(parse_statm_pages("  4096   512  "), Some(512));
    }

    #[test]
    fn parse_statm_rejects_malformed() {
        assert_eq!(parse_statm_pages(""), None);
        assert_eq!(parse_statm_pages("4096"), None);
        assert_eq!(parse_statm_pages("4096 notanumber"), None);
    }
}
