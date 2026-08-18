//! Bounded child-process I/O: captures that cannot exhaust memory, drains
//! that cannot deadlock, and a kill that reaches inherited descriptors.
//!
//! Every subprocess dmux spawns — a `wezterm` probe, an ssh transport leg,
//! a tmux query — is an independent program whose output volume and
//! lifetime this crate does not control.  Three hazards follow from that,
//! and they are answered here once rather than re-derived at each call
//! site.
//!
//! **Unbounded capture.**  `read_to_end` on a child pipe is exactly as
//! large as the child decides to make it.  A wedged, misconfigured, or
//! hostile executable that writes without end turns a diagnostic capture
//! into an out-of-memory abort of the dmux process holding the locks and
//! leases — so every capture carries an explicit byte cap, passed by the
//! call site that knows its real bound ([`bounded_read`]).
//!
//! **The full-pipe deadlock.**  Stopping at the cap is not enough.  A pipe
//! is only a kernel buffer (64 KiB on Linux, 16 KiB on macOS by default),
//! and a child that fills it blocks inside `write(2)` until someone drains
//! the other end.  A parent that stops reading at its cap and then waits
//! for the child to exit waits forever: the child cannot progress to exit,
//! and the parent cannot progress past `wait`.  [`bounded_read`] therefore
//! keeps reading to EOF after its cap has filled and discards the excess,
//! reporting the loss through [`BoundedCapture::truncated`] so callers can
//! say "output exceeded N bytes" instead of silently returning a short
//! buffer as if it were the whole thing.
//!
//! For the same reason each pipe needs its own reader: a parent draining
//! stdout while the child blocks writing stderr is the same deadlock with
//! the descriptors swapped.  Spawn one thread per piped stream.
//!
//! **Inherited descriptors.**  The write end of each pipe is inherited by
//! every descendant the child spawns.  Killing only the direct child leaves
//! a surviving grandchild holding that write end open, so the parent's
//! reader never observes EOF and the drain thread never joins — the
//! deadline that was supposed to bound the operation bounds nothing.
//! Spawning with `Command::process_group(0)` places the child, and by
//! inheritance its descendants, in their own process group;
//! [`kill_process_group`] then signals that entire group, closing the
//! inherited descriptors along with it.
//!
//! A correct bounded capture is the three parts used together: spawn with
//! `process_group(0)`, read each pipe on its own thread with
//! [`bounded_read`], and call [`kill_process_group`] on the deadline and
//! wait-error paths before joining those threads.

use std::io::Read;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Join one [`bounded_read`] thread, or abandon it once `until` has passed.
///
/// `std` has no timed join, and an untimed one is the whole hazard this
/// module exists to answer: a descendant holding an inherited write end
/// keeps the reader blocked in `read`, so a plain `join()` after a deadline
/// would void that deadline. Abandoning the thread costs one bounded buffer,
/// freed when the pipe finally closes; blocking on it costs the guarantee.
///
/// `until` is a shared instant rather than a per-call duration on purpose: a
/// caller joining several readers must bound their *total* wait, not grant
/// each one the full grace.
pub fn join_capture(reader: JoinHandle<BoundedCapture>, until: Instant) -> Option<BoundedCapture> {
    while !reader.is_finished() {
        if Instant::now() >= until {
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    reader.join().ok()
}

/// What [`bounded_read`] retained, and whether anything was dropped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoundedCapture {
    /// The retained prefix, never longer than the cap the caller passed.
    pub bytes: Vec<u8>,
    /// Set when the stream produced more than the cap allowed.  The excess
    /// was drained and discarded, so `bytes` is a strict prefix of what the
    /// child actually wrote: report the truncation rather than presenting
    /// this as the complete output.
    pub truncated: bool,
}

/// Read `reader` to EOF, retaining at most `limit` bytes.
///
/// The reader keeps being drained after the cap fills.  That is the
/// deadlock-avoidance property described in the module docs, not an
/// optimization: abandoning a full pipe leaves the child blocked in
/// `write(2)` and the parent blocked in `wait`.  Bytes past the cap are
/// discarded and [`BoundedCapture::truncated`] is set.
///
/// A read error terminates the capture the same way EOF does, returning
/// whatever was retained so far — a killed child or a closed pipe is a
/// normal outcome on the timeout path, not a case worth losing the
/// diagnostic prefix over.
///
/// Run this on a dedicated thread per piped stream, so stdout and stderr
/// drain concurrently.
pub fn bounded_read(mut reader: impl Read, limit: usize) -> BoundedCapture {
    let mut captured = Vec::with_capacity(limit.min(4096));
    let mut chunk = [0_u8; 4096];
    let mut truncated = false;
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => {
                return BoundedCapture {
                    bytes: captured,
                    truncated,
                };
            }
            Ok(count) => {
                let room = limit.saturating_sub(captured.len());
                captured.extend_from_slice(&chunk[..count.min(room)]);
                if count > room {
                    truncated = true;
                }
            }
        }
    }
}

/// SIGKILL the process group led by `child_pid`, ignoring the result.
///
/// The caller must have spawned that child with
/// `std::os::unix::process::CommandExt::process_group(0)`, which is what
/// makes the child its own group leader and puts every descendant it spawns
/// in the same group.  Signalling the group — rather than the child alone —
/// is what closes pipe write ends inherited by grandchildren, so a bounded
/// reader actually reaches EOF.
///
/// Failure is deliberately ignored: `ESRCH` is the ordinary case where the
/// direct child already exited and left no descendants behind.
pub fn kill_process_group(child_pid: u32) {
    if let Some(target) = group_signal_target(child_pid) {
        // SAFETY: `libc::kill` is a bare syscall wrapper — it dereferences
        // no pointer and touches no memory this process owns, so the entire
        // obligation is that `target` names what the caller intends.
        // `group_signal_target` admits only `1..=pid_t::MAX`, so `target` is
        // a negated live pid: the negation cannot overflow, and it cannot be
        // either of the two dangerous wildcards — `0` (this process's own
        // group, which would kill the caller) or `-1` (every process we are
        // permitted to signal). The caller spawned that child as its own
        // group leader, so the group named here holds exactly that child and
        // its descendants.
        unsafe {
            libc::kill(target, libc::SIGKILL);
        }
    }
}

/// The `kill(2)` target naming the process group led by `child_pid`, or
/// `None` when `child_pid` cannot name one.
///
/// Split out from the `unsafe` block so the guard is unit-testable without
/// signalling anything: a pid too large for `pid_t` and the pid `0` — which
/// would negate to "this process's own group" — both yield `None`.
fn group_signal_target(child_pid: u32) -> Option<libc::pid_t> {
    match libc::pid_t::try_from(child_pid) {
        Ok(pid) if pid > 0 => Some(-pid),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    /// Serves `remaining` bytes in small slices, then EOF, recording what
    /// the caller actually consumed so a test can prove the drain happened.
    struct CountingReader {
        remaining: usize,
        served: usize,
        saw_eof: bool,
        slice: usize,
    }

    impl CountingReader {
        fn new(total: usize, slice: usize) -> Self {
            CountingReader {
                remaining: total,
                served: 0,
                saw_eof: false,
                slice,
            }
        }
    }

    impl io::Read for CountingReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                self.saw_eof = true;
                return Ok(0);
            }
            let count = buf.len().min(self.slice).min(self.remaining);
            for (offset, byte) in buf[..count].iter_mut().enumerate() {
                *byte = ((self.served + offset) % 251) as u8;
            }
            self.remaining -= count;
            self.served += count;
            Ok(count)
        }
    }

    /// Yields `prefix`, then a hard error, so the error path is observable.
    struct FailingReader {
        prefix: Vec<u8>,
    }

    impl io::Read for FailingReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.prefix.is_empty() {
                return Err(io::Error::other("pipe went away"));
            }
            let count = buf.len().min(self.prefix.len());
            buf[..count].copy_from_slice(&self.prefix[..count]);
            self.prefix.drain(..count);
            Ok(count)
        }
    }

    fn pattern(len: usize) -> Vec<u8> {
        (0..len).map(|index| (index % 251) as u8).collect()
    }

    #[test]
    fn capture_under_the_cap_is_complete() {
        let payload = pattern(1000);
        let capture = bounded_read(payload.as_slice(), 4096);
        assert_eq!(capture.bytes, payload);
        assert!(!capture.truncated);
    }

    #[test]
    fn capture_exactly_at_the_cap_is_not_truncated() {
        for total in [1000_usize, 4096, 8192] {
            let payload = pattern(total);
            let capture = bounded_read(payload.as_slice(), total);
            assert_eq!(capture.bytes, payload, "cap {total}");
            assert!(!capture.truncated, "cap {total} must not flag truncation");
        }
    }

    #[test]
    fn one_byte_past_the_cap_flags_truncation() {
        let payload = pattern(4097);
        let capture = bounded_read(payload.as_slice(), 4096);
        assert_eq!(capture.bytes, payload[..4096]);
        assert!(capture.truncated);
    }

    #[test]
    fn past_the_cap_retains_the_prefix_and_flags_truncation() {
        let payload = pattern(50_000);
        let capture = bounded_read(payload.as_slice(), 1000);
        assert_eq!(capture.bytes.len(), 1000);
        assert_eq!(capture.bytes, payload[..1000]);
        assert!(capture.truncated);
    }

    #[test]
    fn a_zero_cap_captures_nothing_but_still_reports_truncation() {
        let mut reader = CountingReader::new(8192, 512);
        let capture = bounded_read(&mut reader, 0);
        assert!(capture.bytes.is_empty());
        assert!(capture.truncated);
        assert_eq!(reader.served, 8192);
        assert!(reader.saw_eof);
    }

    #[test]
    fn an_empty_stream_is_neither_captured_nor_truncated() {
        let capture = bounded_read(b"".as_slice(), 4096);
        assert!(capture.bytes.is_empty());
        assert!(!capture.truncated);
    }

    #[test]
    fn reading_continues_to_eof_past_the_cap() {
        // The anti-deadlock property: everything the stream offers is
        // consumed, even though only the first 16 bytes are kept.
        let total = 40 * 1024;
        let mut reader = CountingReader::new(total, 700);
        let capture = bounded_read(&mut reader, 16);
        assert_eq!(capture.bytes, pattern(16));
        assert!(capture.truncated);
        assert_eq!(reader.served, total, "reader must be drained to EOF");
        assert!(reader.saw_eof, "reader must be read until it returns Ok(0)");
    }

    #[test]
    fn a_read_error_ends_the_capture_with_what_it_had() {
        let capture = bounded_read(
            FailingReader {
                prefix: pattern(64),
            },
            4096,
        );
        assert_eq!(capture.bytes, pattern(64));
        assert!(!capture.truncated);
    }

    #[test]
    fn a_child_writing_past_the_pipe_buffer_does_not_deadlock() {
        // 256 KiB exceeds the pipe buffer on both Linux and macOS, so a
        // capture that stopped reading at its cap would leave `dd` blocked
        // in write(2) forever and this `wait` would never return.
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg("dd if=/dev/zero bs=1024 count=256 2>/dev/null")
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn /bin/sh");
        let stdout = child.stdout.take().expect("piped stdout");
        let reader = std::thread::spawn(move || bounded_read(stdout, 2048));
        let status = child.wait().expect("wait for /bin/sh");
        let capture = reader.join().expect("reader thread");
        assert!(status.success(), "child exited: {status:?}");
        assert_eq!(capture.bytes.len(), 2048);
        assert!(capture.truncated);
    }

    #[test]
    fn kill_process_group_closes_a_pipe_held_by_a_grandchild() {
        // The direct child exits immediately but leaves a backgrounded
        // grandchild holding the inherited stdout write end. Only a
        // group-wide signal closes it; killing the direct child alone would
        // leave the reader below blocked until the `sleep` expired.
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg("sleep 20 & exit 0")
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn /bin/sh");
        let stdout = child.stdout.take().expect("piped stdout");
        let reader = std::thread::spawn(move || bounded_read(stdout, 64));
        let status = child.wait().expect("wait for /bin/sh");
        assert!(status.success(), "direct child exits at once: {status:?}");

        kill_process_group(child.id());
        let capture = reader.join().expect("reader thread");
        assert!(capture.bytes.is_empty());
        assert!(!capture.truncated);
    }

    #[test]
    fn only_a_live_child_pid_becomes_a_signal_target() {
        // A pid that cannot be represented must not be truncated into a
        // signal aimed at some unrelated group, and pid 0 must never become
        // the `kill(0, ...)` that would take down our own process group.
        assert_eq!(group_signal_target(0), None);
        assert_eq!(group_signal_target(u32::MAX), None);
        let past_pid_t = u32::try_from(libc::pid_t::MAX).expect("pid_t::MAX fits u32") + 1;
        assert_eq!(group_signal_target(past_pid_t), None);
        assert_eq!(group_signal_target(4321), Some(-4321));
        assert_eq!(
            group_signal_target(u32::try_from(libc::pid_t::MAX).expect("fits")),
            Some(-libc::pid_t::MAX)
        );
    }

    #[test]
    fn an_unrepresentable_pid_is_a_no_op() {
        kill_process_group(u32::MAX);
    }
}
