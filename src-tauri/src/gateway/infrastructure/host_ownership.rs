//! Private Unix file lease for the macOS desktop service-label owner.
use crate::gateway::application::{DesktopHostClaim, DesktopHostLease};
use nessa_local_storage::{create_directory, open, OpenMode};
use std::{fs::File, io, os::fd::AsRawFd, path::Path};

struct NativeHostLease(File);

impl Drop for NativeHostLease {
    fn drop(&mut self) {
        // flock is shared with a forked descriptor. Closing ours alone could
        // leave the child holding the claim; explicitly release this description.
        if unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) } != 0 {
            eprintln!(
                "[nessa] could not release desktop host ownership: {}",
                io::Error::last_os_error()
            );
        }
    }
}

/// H7–H10: private open, a stable inode, nonblocking claim, no unlink on release.
pub(super) fn try_claim(directory: &Path) -> io::Result<DesktopHostClaim> {
    create_directory(directory)?;
    let file = open(&directory.join("desktop-host.lock"), OpenMode::OpenOrCreate)?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(DesktopHostClaim::Acquired(DesktopHostLease::new(
            NativeHostLease(file),
        )));
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::WouldBlock {
        Ok(DesktopHostClaim::Busy)
    } else {
        Err(error)
    }
}

#[cfg(test)]
#[path = "../../../tests/gateway/infrastructure/host_ownership.rs"]
mod tests;
