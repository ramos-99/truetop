//! End-to-end I/O wait against the kernel's own accounting: a thread does
//! O_DIRECT reads (real, uncached device I/O → uninterruptible sleep), and
//! truetop's IO% must track the block-I/O delay the kernel records in
//! `/proc/<pid>/stat` (`delayacct_blkio_ticks`, the source iotop reads).
//! Needs a real block device and faithful timing, so it runs on the host, not in
//! the vmtest matrix. Run: `cargo xtask test`.

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

#[test]
#[ignore = "needs root + a live kernel; run: cargo xtask test"]
fn io_wait_tracks_kernel_blkio_delay() -> Result<()> {
    // The oracle: enable delay accounting (off by default since 5.14).
    let _ = fs::write("/proc/sys/kernel/task_delayacct", "1");

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
