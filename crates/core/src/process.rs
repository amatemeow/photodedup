//! Running a child process that a scan can still cancel.
//!
//! `Command::output()` cannot be interrupted. During a scan ffprobe and ffmpeg
//! can block for minutes: opening a file that exists only in cloud storage
//! makes the sync client download it first. A cancel that waited for them let
//! the Cancel button look broken, so scan-time calls use [`output_cancellable`],
//! which kills the child as soon as the scan is cancelled.

use std::io::{self, Read};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::Duration;

/// `Command::output()`, except that once `cancelled` returns true the child is
/// killed and `ErrorKind::Interrupted` is returned straight away, without
/// waiting for the child to exit (a process stuck in a file read can take a
/// moment to die, and the cancel must not wait for it).
///
/// With `cancelled` as `None` this is exactly `Command::output()`.
pub fn output_cancellable(cmd: &mut Command, cancelled: Option<&(dyn Fn() -> bool + Sync)>) -> io::Result<Output> {
    let Some(cancelled) = cancelled else {
        return cmd.output();
    };
    if cancelled() {
        return Err(interrupted());
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;

    // Drain both pipes on their own threads, so a child that writes a lot can
    // never fill a pipe and stall while we are only watching for its exit.
    let mut stdout = child.stdout.take().expect("stdout was piped");
    let mut stderr = child.stderr.take().expect("stderr was piped");
    let out = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let err = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        bytes
    });

    // Poll with a short, growing pause: a child that finishes in a few
    // milliseconds (most ffprobe calls) is noticed almost at once, and one that
    // runs for minutes costs a check every 25 ms.
    let mut pause = Duration::from_millis(1);
    loop {
        if let Some(status) = child.try_wait()? {
            let stdout = out.join().unwrap_or_default();
            let stderr = err.join().unwrap_or_default();
            return Ok(Output { status, stdout, stderr });
        }
        if cancelled() {
            let _ = child.kill();
            // Reap it, and let the pipe readers end, off to the side: the
            // caller is told about the cancel now.
            thread::spawn(move || {
                let _ = child.wait();
                let _ = out.join();
                let _ = err.join();
            });
            return Err(interrupted());
        }
        thread::sleep(pause);
        pause = (pause * 2).min(Duration::from_millis(25));
    }
}

fn interrupted() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "cancelled")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    fn sh(script: &str) -> Command {
        let mut c = Command::new("sh");
        c.arg("-c").arg(script);
        c
    }

    #[test]
    fn collects_output_and_status_like_command_output() {
        let never = || false;
        let out = output_cancellable(&mut sh("printf out; printf err >&2; exit 3"), Some(&never)).unwrap();
        assert_eq!((out.stdout.as_slice(), out.stderr.as_slice()), (&b"out"[..], &b"err"[..]));
        assert_eq!(out.status.code(), Some(3));
        let plain = output_cancellable(&mut sh("printf x"), None).unwrap();
        assert_eq!(plain.stdout, b"x");
    }

    #[test]
    fn a_child_that_writes_more_than_a_pipe_holds_does_not_stall() {
        let never = || false;
        let out = output_cancellable(&mut sh("head -c 2000000 /dev/zero"), Some(&never)).unwrap();
        assert_eq!(out.stdout.len(), 2_000_000);
    }

    #[test]
    fn cancelling_returns_at_once_and_the_child_does_not_outlive_it() {
        let flag = AtomicBool::new(false);
        let cancelled = || flag.load(Ordering::Relaxed);
        let pid_file = tempfile::NamedTempFile::new().unwrap();
        let script = format!("echo $$ > {}; sleep 30", pid_file.path().display());
        let start = Instant::now();
        let result = thread::scope(|s| {
            s.spawn(|| {
                thread::sleep(Duration::from_millis(300));
                flag.store(true, Ordering::Relaxed);
            });
            output_cancellable(&mut sh(&script), Some(&cancelled))
        });
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
        assert!(start.elapsed() < Duration::from_secs(3), "took {:?}", start.elapsed());

        // The 30 s sleep is gone shortly after (it was killed, not left to finish).
        let pid: i32 = std::fs::read_to_string(pid_file.path()).unwrap().trim().parse().unwrap();
        let mut alive = true;
        for _ in 0..50 {
            // signal 0 only asks whether the process exists
            alive = unsafe { libc::kill(pid, 0) } == 0;
            if !alive {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive, "the killed child (pid {pid}) is still running");
    }

    #[test]
    fn already_cancelled_never_starts_the_child() {
        let always = || true;
        let marker = tempfile::NamedTempFile::new().unwrap();
        let script = format!("echo started > {}", marker.path().display());
        assert_eq!(output_cancellable(&mut sh(&script), Some(&always)).unwrap_err().kind(), io::ErrorKind::Interrupted);
        assert_eq!(std::fs::read_to_string(marker.path()).unwrap(), "", "the child ran although the scan was already cancelled");
    }
}
