use std::fmt;
use std::io::{Read, Write};
use std::time::Duration;

use reqwest::redirect::Policy;

use crate::agent_install::application::{ArchiveSource, SourceFailure, StagedArchive};

/// How long to wait for the other end to answer at all.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// The longest a single archive may take to arrive.
///
/// The blocking client has one timeout and it covers reading the body, so this
/// is a ceiling on the whole transfer rather than a limit on silence. It has to
/// be generous: a release archive is around a hundred megabytes, which is
/// twenty minutes on a connection doing 100 KB/s, and someone on a bad
/// connection should still end up with a working agent. It is here so that a
/// transfer which has genuinely stopped eventually fails instead of holding the
/// install open forever — not to police how slow is too slow.
///
/// The client's own default is thirty seconds, which would fail almost every
/// real download, so this is set explicitly rather than left alone.
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(45 * 60);

/// How many hops a redirect chain may take before it is treated as a loop.
const REDIRECT_LIMIT: usize = 10;

/// The most bytes an agent runtime archive may be.
///
/// Around five times the largest pinned archive, so it is not a budget anybody
/// has to think about — it is there so that a server which answers a hundred
/// megabyte request with an endless body fills a disk with a failure instead of
/// with an archive. Nothing has been measured at this point, so this is not a
/// judgement about the bytes; it is a bound on how many of them are kept.
const MAXIMUM_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;

/// How much is moved between the socket and the disk at a time.
const TRANSFER_CHUNK: usize = 64 * 1024;

/// Why this process has no HTTPS client to fetch archives with.
///
/// Its own type rather than a [`SourceFailure`]: nothing has been asked for
/// yet, so this is not a release that could not be reached — it is a client
/// that could not be built, which usually means the TLS backend found no trust
/// store to work from. Kept apart so that a machine with no usable certificate
/// store is not reported as an unreachable registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoHttpsClient(String);

impl fmt::Display for NoHttpsClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "could not start an https client: {}", self.0)
    }
}

impl std::error::Error for NoHttpsClient {}

/// Release archives, fetched over HTTPS.
///
/// Blocking on purpose. This runs from the `install-agent` command, which is a
/// short-lived process with nothing else to do, and the work it coordinates —
/// hashing and unpacking a hundred megabytes — is blocking anyway. It must not
/// be called from inside the server's async runtime: the underlying blocking
/// client panics there rather than deadlocking quietly, which is the better of
/// the two but still a panic.
pub struct HttpsArchives {
    client: reqwest::blocking::Client,
}

impl HttpsArchives {
    /// A client that stays on HTTPS, follows a bounded redirect chain, and
    /// verifies certificates.
    ///
    /// Redirects are followed because the npm registry serves archives from a
    /// CDN host. They are also confined to HTTPS: the pin's own rule is that an
    /// archive is fetched over HTTPS, and a redirect to plain HTTP would undo
    /// that at the last moment, in a place the pin cannot see. The digest check
    /// afterwards is what makes following a redirect safe at all, since a
    /// redirect to the wrong thing produces the wrong hash and installs
    /// nothing — but it is the second line here, not the first.
    ///
    /// Both policies are set rather than inherited. They are reqwest's defaults
    /// today, and a default is not a decision this install can rest on.
    pub fn new() -> Result<Self, NoHttpsClient> {
        reqwest::blocking::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(TRANSFER_TIMEOUT)
            .https_only(true)
            .redirect(Policy::limited(REDIRECT_LIMIT))
            .user_agent(concat!("nessa/", env!("CARGO_PKG_VERSION")))
            .build()
            .map(|client| Self { client })
            .map_err(|error| NoHttpsClient(error.to_string()))
    }
}

impl ArchiveSource for HttpsArchives {
    fn download(&self, url: &str, staged: &mut StagedArchive) -> Result<(), SourceFailure> {
        let mut response = self
            .client
            .get(url)
            .send()
            .map_err(|error| SourceFailure::Unreachable(error.to_string()))?;
        if let Some(refusal) = refusal(response.status().as_u16()) {
            return Err(refusal);
        }
        store(&mut response, staged, MAXIMUM_ARCHIVE_BYTES)
    }
}

/// Whether a status is an answer that says no.
///
/// Its own function so that "which statuses are a refusal" is one decision with
/// a test on it, rather than a condition inside a method that only a live
/// download reaches.
fn refusal(status: u16) -> Option<SourceFailure> {
    (!(200..300).contains(&status)).then_some(SourceFailure::Refused(status))
}

/// Move a response body onto the staged file, bounded.
///
/// Streamed straight to disk. Nothing here decides whether these bytes are
/// trustworthy — the caller hashes them afterwards — so this only has to
/// deliver them intact or say which half of the job failed.
///
/// Copied by hand rather than with `io::copy` for exactly that: a socket that
/// stopped answering is the network's doing and a write that would not complete
/// is this machine's, and telling somebody with a full disk to check their
/// connection sends them to look at the wrong thing.
///
/// `limit` is passed in rather than read from the constant so that the bound
/// can be exercised by a test without moving half a gigabyte.
fn store(
    body: &mut impl Read,
    staged: &mut StagedArchive,
    limit: u64,
) -> Result<(), SourceFailure> {
    let mut buffer = vec![0u8; TRANSFER_CHUNK];
    let mut written: u64 = 0;
    loop {
        let read = body
            .read(&mut buffer)
            .map_err(|error| SourceFailure::Unreachable(error.to_string()))?;
        if read == 0 {
            return Ok(());
        }
        written += read as u64;
        if written > limit {
            return Err(SourceFailure::TooLarge(limit));
        }
        staged
            .file_mut()
            .write_all(&buffer[..read])
            .map_err(|error| SourceFailure::NotStored(error.to_string()))?;
    }
}

#[cfg(test)]
#[path = "../../../tests/agent_install/https_archives.rs"]
mod tests;
