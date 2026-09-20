//! The gateway log's size bound.
//!
//! launchd opens `~/.nessa/logs/gateway.log` itself — that is what
//! `StandardOutPath` and `StandardErrorPath` mean — and hands this process the
//! already-open descriptor as its stdout and stderr. Nothing in here can make
//! launchd open it again, and that is what decides how the file may be rotated.
//!
//! Renaming the log would leave every descriptor already open on it — ours, and
//! launchd's own — pointing at the renamed inode. This run's lines would land in
//! the *previous* file, and `gateway.log` would not exist again until the next
//! spawn. So the contents are copied aside and the log is then emptied in place:
//! the inode is the same one afterwards, and both descriptors go on writing to
//! the name a person knows. Verified against launchd on macOS 26 (Darwin 25.6):
//! it opens those paths with `O_APPEND` — `F_GETFL` reports `0xa` on fds 1 and
//! 2 of a launched job — so the first write after the emptying lands at the new
//! end of the file rather than back at the old offset, leaving no hole behind.
//!
//! Only the process whose own stderr *is* that file rotates it, checked by
//! device and inode rather than by path. A `nessa` command run in a terminal,
//! or a second server refused by the registry lock, writes its lines somewhere
//! else and has no business emptying a log it is not filling.
//!
//! ```text
//! launchd ──open(O_APPEND)──► gateway.log ◄── this process's stderr
//!                                  │
//!                       start ──► over the limit?
//!                                  │
//!                       copy to gateway.log.1, then set_len(0)
//!                                  └── same inode, both descriptors intact
//! ```
//!
//! This happens once, at a start, before the first line is written. A service
//! that is being restarted every few seconds is bounded by that. A single
//! long-lived run can still grow past the limit before its next start: the
//! alternative is a thread watching the file, and a healthy gateway logs a
//! handful of lines an hour.
use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// The gateway's log, inside the stage's log directory. launchd's
/// `StandardOutPath` and `StandardErrorPath` name this same file, so the host
/// and the server have to agree on it.
pub(super) const GATEWAY_LOG: &str = "gateway.log";

/// How large `gateway.log` may be at a start. Twelve hours of the crash loop
/// this bound exists for is roughly a megabyte, so four is several days of the
/// worst case and still small enough to send to us.
const LIMIT: u64 = 4 * 1024 * 1024;

/// What a start did to the log.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Rotation {
    /// Under the limit, or not a file this process is writing into.
    Kept,
    /// The contents are in the previous file and the log starts again empty.
    Rolled,
}

/// The name the previous contents are kept under, beside the log itself.
fn previous(log: &Path) -> PathBuf {
    let mut name = log.file_name().unwrap_or_default().to_os_string();
    name.push(".1");
    log.with_file_name(name)
}

/// Empty `log` into its previous file if this process is writing into it and it
/// has reached the limit.
///
/// A log that is not there, or that belongs to someone else, is not a failure:
/// there is simply nothing to bound. Everything else is reported so the caller
/// can say that the bound did not hold, rather than a start failing over a log.
pub(super) fn bound(log: &Path) -> io::Result<Rotation> {
    rotate(log, io::stderr().as_fd(), LIMIT)
}

fn rotate(log: &Path, output: BorrowedFd<'_>, limit: u64) -> io::Result<Rotation> {
    let mut file = match OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(log)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Rotation::Kept),
        Err(error) => return Err(error),
    };
    let ours = File::from(output.try_clone_to_owned()?).metadata()?;
    let log_metadata = file.metadata()?;
    if (ours.dev(), ours.ino()) != (log_metadata.dev(), log_metadata.ino())
        || log_metadata.len() < limit
    {
        return Ok(Rotation::Kept);
    }
    // Copied before it is emptied, so an interruption costs a duplicate of the
    // previous file rather than the log itself.
    let mut kept = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(previous(log))?;
    file.seek(SeekFrom::Start(0))?;
    io::copy(&mut file, &mut kept)?;
    kept.sync_all()?;
    file.set_len(0)?;
    Ok(Rotation::Rolled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A file standing in for what launchd hands the process: opened for
    /// append, the way `StandardErrorPath` is.
    fn appending(path: &Path) -> File {
        OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .expect("log")
    }
    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    /// The boundary itself: a log one byte short of the limit is left alone,
    /// and one that has reached it is rolled. Off by one here is a log that
    /// grows for another whole run.
    #[test]
    fn the_limit_is_the_size_a_start_will_no_longer_leave_alone() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let log = directory.path().join("gateway.log");
        let stderr = appending(&log);
        for (size, expected) in [
            (0, Rotation::Kept),
            (99, Rotation::Kept),
            (100, Rotation::Rolled),
            (101, Rotation::Rolled),
        ] {
            std::fs::write(&log, "x".repeat(size)).expect("log contents");
            assert_eq!(
                rotate(&log, stderr.as_fd(), 100).expect("rotation"),
                expected,
                "at {size} bytes"
            );
        }
    }

    /// What rotation has to leave behind: the previous contents under a name
    /// of their own, an empty log, and the same file the process is writing
    /// into — not a new one beside an orphaned descriptor.
    #[test]
    fn the_emptied_log_is_still_the_file_the_process_is_writing_into() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let log = directory.path().join("gateway.log");
        std::fs::write(&log, "older run\n".repeat(20)).expect("log contents");
        let mut stderr = appending(&log);
        let inode = stderr.metadata().expect("metadata").ino();

        assert_eq!(
            rotate(&log, stderr.as_fd(), 100).expect("rotation"),
            Rotation::Rolled
        );

        assert_eq!(read(&previous(&log)), "older run\n".repeat(20));
        assert_eq!(read(&log), "");
        // The descriptor launchd handed us is still open on the same file, and
        // appending through it lands at the new beginning rather than back at
        // the old offset.
        writeln!(stderr, "this run").expect("write");
        assert_eq!(read(&log), "this run\n");
        assert_eq!(
            std::fs::metadata(&log).expect("metadata").ino(),
            inode,
            "rotation must not replace the inode the open descriptors hold"
        );
        // One previous file, not a generation of them.
        let names: Vec<_> = std::fs::read_dir(directory.path())
            .expect("directory")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
    }

    /// A second rotation replaces the previous file rather than accumulating.
    #[test]
    fn only_one_previous_file_is_kept() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let log = directory.path().join("gateway.log");
        let stderr = appending(&log);
        for run in ["first", "second"] {
            std::fs::write(&log, run.repeat(40)).expect("log contents");
            assert_eq!(
                rotate(&log, stderr.as_fd(), 100).expect("rotation"),
                Rotation::Rolled
            );
        }
        assert_eq!(read(&previous(&log)), "second".repeat(40));
    }

    /// The log of a gateway that is running is not this process's to empty:
    /// a `nessa` command in a terminal writes somewhere else entirely.
    #[test]
    fn a_log_this_process_is_not_writing_into_is_left_alone() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let log = directory.path().join("gateway.log");
        std::fs::write(&log, "another process\n".repeat(40)).expect("log contents");
        let elsewhere = appending(&directory.path().join("terminal"));
        assert_eq!(
            rotate(&log, elsewhere.as_fd(), 100).expect("rotation"),
            Rotation::Kept
        );
        assert_eq!(read(&log), "another process\n".repeat(40));
        assert!(!previous(&log).exists());
    }

    /// A first run has no log yet, which is not something to report.
    #[test]
    fn a_log_that_is_not_there_yet_is_not_a_failure() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let log = directory.path().join("gateway.log");
        let elsewhere = appending(&directory.path().join("terminal"));
        assert_eq!(
            rotate(&log, elsewhere.as_fd(), 100).expect("rotation"),
            Rotation::Kept
        );
    }

    /// A symbolic link where the log should be is refused rather than
    /// followed: the log's directory is private, and a link in it is not
    /// something to empty on a stranger's behalf.
    #[test]
    fn a_link_in_place_of_the_log_is_refused() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let target = directory.path().join("elsewhere");
        std::fs::write(&target, "x".repeat(400)).expect("target");
        let log = directory.path().join("gateway.log");
        std::os::unix::fs::symlink(&target, &log).expect("link");
        let stderr = appending(&target);
        let error = rotate(&log, stderr.as_fd(), 100).expect_err("must refuse a link");
        assert_ne!(error.kind(), io::ErrorKind::NotFound, "{error}");
        assert_eq!(read(&target).len(), 400);
    }
}
