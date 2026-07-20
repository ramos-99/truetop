//! I/O-wait integration tests, run by `cargo xtask test` (root + a live kernel).
//!
//! `io_wait_tracks_kernel_blkio_delay` - the end-to-end oracle: a thread does
//! O_DIRECT reads (real, uncached device I/O → uninterruptible sleep) and
//! truetop's IO% must track the block-I/O delay the kernel records in
//! `/proc/<pid>/stat` (`delayacct_blkio_ticks`, the source iotop reads). It needs
//! a real block device, so it skips where there is none.
//!
//! `attaches_and_collects_on_this_kernel` - the cross-kernel smoke the vmtest
//! matrix runs on every kernel and distro: no device needed, it proves truetop
//! loads, attaches, and collects across the CO-RE and tracepoint ABI surface.

use std::{
    fs,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail};
use truetop::attach;

const SCRATCH_BYTES: usize = 64 << 20;
const BLOCK: usize = 1 << 20;
const ALIGN: usize = 4096;

// Lives in the working directory, not /tmp: O_DIRECT needs a real filesystem
// and /tmp is commonly tmpfs.
struct Scratch(PathBuf);

impl Scratch {
    fn create() -> Result<Self> {
        let path = PathBuf::from("iowait-scratch.tmp");
        fs::write(&path, vec![0u8; SCRATCH_BYTES])?;
        fs::File::open(&path)?.sync_all()?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Loop O_DIRECT reads over the file: every block bypasses the page cache and
/// blocks this thread uninterruptibly on the device.
fn direct_read_loop(path: &Path, stop: &AtomicBool) -> Result<()> {
    use std::os::unix::ffi::OsStrExt as _;

    let cpath = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: a valid NUL-terminated path.
    let fd = unsafe { libc::open(cpath.as_ptr(), libc::O_RDONLY | libc::O_DIRECT) };
    if fd < 0 {
        bail!("open O_DIRECT: {}", std::io::Error::last_os_error());
    }
    // SAFETY: open returned a fresh, owned descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };

    let mut buf = vec![0u8; BLOCK + ALIGN];
    let aligned = unsafe { buf.as_mut_ptr().add(buf.as_ptr().align_offset(ALIGN)) };
    let mut offset: i64 = 0;
    while !stop.load(Ordering::Relaxed) {
        // SAFETY: `aligned` points at BLOCK writable bytes, ALIGN-aligned.
        let n = unsafe { libc::pread64(fd.as_raw_fd(), aligned.cast(), BLOCK, offset) };
        if n < 0 {
            bail!("pread64: {}", std::io::Error::last_os_error());
        }
        offset = if n == 0 { 0 } else { offset + n as i64 };
    }
    Ok(())
}

/// Kernel block-I/O delay for the whole process (summed over its threads), in
/// clock ticks — `/proc/<pid>/task/<tid>/stat` field 42.
fn blkio_ticks(pid: u32) -> Result<u64> {
    let mut total: u64 = 0;
    for entry in fs::read_dir(format!("/proc/{pid}/task"))? {
        let tid = entry?.file_name();
        let stat = fs::read_to_string(format!("/proc/{pid}/task/{}/stat", tid.to_string_lossy()))?;
        let after_comm = stat.rsplit_once(')').context("malformed stat")?.1;
        // After the comm the fields start at field 3, so field 42 is index 39.
        if let Some(ticks) = after_comm
            .split_whitespace()
            .nth(39)
            .and_then(|t| t.parse::<u64>().ok())
        {
            total += ticks;
        }
    }
    Ok(total)
}

fn clk_tck() -> f64 {
    // SAFETY: sysconf with a valid name is always safe to call.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if hz > 0 { hz as f64 } else { 100.0 }
}

/// Whether `path` is on a real block-backed filesystem. The oracle needs one;
/// the vmtest rootfs (9p or virtiofs) and tmpfs are not. Allowlist the common
/// on-disk filesystems - anything else means skip.
fn is_block_backed(path: &Path) -> Result<bool> {
    use std::os::unix::ffi::OsStrExt as _;

    const EXT_MAGIC: i64 = 0xEF53; // ext2/3/4
    const XFS_MAGIC: i64 = 0x5846_5342;
    const BTRFS_MAGIC: i64 = 0x9123_683E;
    const F2FS_MAGIC: i64 = 0xF2F5_2010;

    let cpath = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: statfs fills a zeroed buffer given a valid NUL-terminated path.
    let mut buf: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(cpath.as_ptr(), &mut buf) } != 0 {
        bail!(
            "statfs {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        );
    }
    Ok(matches!(
        buf.f_type,
        EXT_MAGIC | XFS_MAGIC | BTRFS_MAGIC | F2FS_MAGIC
    ))
}

#[test]
#[ignore = "needs root + a live kernel; run: cargo xtask test"]
fn io_wait_tracks_kernel_blkio_delay() -> Result<()> {
    // The oracle: enable delay accounting (off by default since 5.14).
    let _ = fs::write("/proc/sys/kernel/task_delayacct", "1");

    // Needs a real block device; the smoke test covers device-less kernels.
    if !is_block_backed(Path::new("."))? {
        eprintln!("io_wait: skipping, cwd is not on a block-backed filesystem");
        return Ok(());
    }

    let (_ebpf, mut collector) = attach()?;
    let scratch = Scratch::create()?;

    let stop = Arc::new(AtomicBool::new(false));
    let reader = {
        let stop = Arc::clone(&stop);
        let path = scratch.0.clone();
        thread::spawn(move || direct_read_loop(&path, &stop))
    };
    let me = std::process::id();

    collector.tick();
    let ticks0 = blkio_ticks(me)?;
    let t0 = Instant::now();

    thread::sleep(Duration::from_secs(3));

    let ticks1 = blkio_ticks(me)?;
    let elapsed = t0.elapsed().as_secs_f64();
    let snapshot = collector.tick();

    stop.store(true, Ordering::Relaxed);
    reader.join().expect("reader thread panicked")?;

    let truetop = snapshot
        .processes
        .iter()
        .find(|p| p.pid == me)
        .and_then(|p| p.io)
        .map(|io| io.io_wait_percent / 100.0)
        .context("this process not seen by truetop")?;
    let kernel = (ticks1 - ticks0) as f64 / clk_tck() / elapsed;

    assert!(
        kernel > 0.05,
        "kernel recorded no block-I/O delay: {kernel:.2}"
    );
    assert!(truetop > 0.05, "truetop recorded no I/O wait: {truetop:.2}");
    assert!(
        (truetop - kernel).abs() < 0.5,
        "truetop {truetop:.2} vs kernel blkio {kernel:.2}"
    );
    Ok(())
}

/// Cross-kernel smoke: truetop loads, attaches, and collects on this kernel -
/// what the vmtest matrix checks on every kernel/distro. No block device needed.
#[test]
#[ignore = "needs root + a live kernel; run: cargo xtask test"]
fn attaches_and_collects_on_this_kernel() -> Result<()> {
    let (_ebpf, mut collector) = attach()?;
    let me = std::process::id();

    // Seed deltas, burn a little CPU so this process is scheduled with recorded
    // time, then sample - it must then see itself.
    collector.tick();
    let mut x = 0u64;
    for i in 0..100_000_000u64 {
        x = x.wrapping_add(i.rotate_left(3));
    }
    std::hint::black_box(x);
    let snapshot = collector.tick();

    assert!(
        snapshot.processes.iter().any(|p| p.pid == me),
        "truetop did not see its own process ({me}) after attaching"
    );
    Ok(())
}
