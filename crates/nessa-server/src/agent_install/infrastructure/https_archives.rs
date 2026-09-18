use std::fs::File;
use std::io;
use std::path::Path;
use std::time::Duration;

use crate::agent_install::application::{ArchiveSource, SourceFailure};

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
    /// A client that will follow redirects and verify certificates.
    ///
    /// Redirects are followed because the npm registry serves archives from a
    /// CDN host; the digest check afterwards is what makes that safe, since a
    /// redirect to the wrong thing produces the wrong hash and installs
    /// nothing.
    pub fn new() -> Result<Self, SourceFailure> {
        reqwest::blocking::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(TRANSFER_TIMEOUT)
            .user_agent(concat!("nessa/", env!("CARGO_PKG_VERSION")))
            .build()
            .map(|client| Self { client })
            .map_err(|error| SourceFailure::Unreachable(error.to_string()))
    }
}

impl ArchiveSource for HttpsArchives {
    fn download(&self, url: &str, destination: &Path) -> Result<(), SourceFailure> {
        let mut response = self
            .client
            .get(url)
            .send()
            .map_err(|error| SourceFailure::Unreachable(error.to_string()))?;
        if !response.status().is_success() {
            return Err(SourceFailure::Refused(response.status().as_u16()));
        }
        let mut file = File::create(destination)
            .map_err(|error| SourceFailure::Unreachable(error.to_string()))?;
        // Streamed straight to disk. The caller hashes the file afterwards, so
        // nothing here decides whether these bytes are trustworthy — this type
        // only has to deliver them intact or say that it could not.
        io::copy(&mut response, &mut file)
            .map_err(|error| SourceFailure::Unreachable(error.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../tests/agent_install/https_archives.rs"]
mod tests;
