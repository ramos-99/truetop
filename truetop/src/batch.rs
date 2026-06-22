//! Raw `BPF_MAP_LOOKUP_BATCH` for the per-CPU `u32 → u64` CPU-time map. Aya has
//! no batch API; this reads the whole map in O(1) syscalls instead of
//! `Map::iter`'s O(N) `get_next_key` + lookup. See bench/BENCHMARKS.md.

use std::{collections::HashMap, io, os::fd::RawFd};

const LOOKUP_BATCH: libc::c_long = 24; // BPF_MAP_LOOKUP_BATCH (uapi/linux/bpf.h)
const BATCH: usize = 4096;

/// Reusable scratch so a tick never allocates (CLAUDE.md §3).
pub struct BatchReader {
    nr_cpus: usize,
    keys: Box<[u32]>,
    values: Box<[u64]>,
}

impl BatchReader {
    /// `nr_cpus` must be the *possible* CPU count (`aya::util::nr_cpus`): the
    /// kernel writes one value slot per possible CPU per key.
    pub fn new(nr_cpus: usize) -> Self {
        Self {
            nr_cpus,
            keys: vec![0; BATCH].into_boxed_slice(),
            values: vec![0; BATCH * nr_cpus].into_boxed_slice(),
        }
    }

    /// Read every entry, summing each key's per-CPU slots into `tgid → ns`.
    pub fn sum_per_cpu(&mut self, fd: RawFd) -> HashMap<u32, u64> {
        let mut totals = HashMap::new();
        let mut cursor = Cursor::default();
        while let Some(count) = self.next_batch(fd, &mut cursor) {
            for i in 0..count {
                let slots = &self.values[i * self.nr_cpus..(i + 1) * self.nr_cpus];
                totals.insert(self.keys[i], slots.iter().sum());
            }
            if count < BATCH {
                break;
            }
        }
        totals
    }

    /// One batch call; returns the entries written, or `None` once exhausted.
    fn next_batch(&mut self, fd: RawFd, cursor: &mut Cursor) -> Option<usize> {
        let mut attr = BatchAttr::new(fd, cursor, &mut self.keys, &mut self.values);
        // SAFETY: the key/value buffers are sized BATCH × nr_cpus, exactly what
        // the kernel writes for this per-CPU map; `attr` is a valid bpf_attr.
        let ret = unsafe {
            libc::syscall(
                libc::SYS_bpf,
                LOOKUP_BATCH,
                &raw mut attr,
                size_of::<BatchAttr>(),
            )
        };
        cursor.started = true;

        // Entries are valid on success and on the ENOENT tail; any other error
        // leaves `count` meaningless, so report exhaustion.
        let usable = ret == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT);
        (usable && attr.count > 0).then_some(attr.count as usize)
    }
}

/// The opaque batch token: NULL on the first call, then the kernel's cursor.
#[derive(Default)]
struct Cursor {
    token: u64,
    started: bool,
}

impl Cursor {
    fn in_ptr(&self) -> u64 {
        if self.started {
            &raw const self.token as u64
        } else {
            0
        }
    }

    fn out_ptr(&mut self) -> u64 {
        &raw mut self.token as u64
    }
}

/// The `batch` variant of `union bpf_attr` (uapi/linux/bpf.h).
#[repr(C)]
#[derive(Default)]
struct BatchAttr {
    in_batch: u64,
    out_batch: u64,
    keys: u64,
    values: u64,
    count: u32,
    map_fd: u32,
    elem_flags: u64,
    flags: u64,
}

impl BatchAttr {
    fn new(fd: RawFd, cursor: &mut Cursor, keys: &mut [u32], values: &mut [u64]) -> Self {
        Self {
            in_batch: cursor.in_ptr(),
            out_batch: cursor.out_ptr(),
            keys: keys.as_mut_ptr() as u64,
            values: values.as_mut_ptr() as u64,
            count: BATCH as u32,
            map_fd: fd as u32,
            ..Default::default()
        }
    }
}
