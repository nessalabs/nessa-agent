use std::fmt;

/// What a pinned release can be wrong about, at the moment it is described.
///
/// Every variant is a fault in the pin itself — a value checked into this
/// repository — rather than anything a user did. They are separate variants
/// because the pin is also what a future release-bumping script will write, and
/// "the digest is not a digest" and "the executable escapes the archive" are
/// different mistakes to make and different ones to report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinRejected {
    /// A version that cannot also be a directory name: empty, or carrying a
    /// path separator, whitespace, or a relative-path segment.
    Version(String),
    /// An operating system or architecture token that is empty or not a plain
    /// lowercase identifier.
    Platform(String),
    /// Not sixty-four lowercase hexadecimal characters.
    Digest(String),
    /// Not `https://`. Plain HTTP would put the archive on the wire for anyone
    /// to replace, and the digest below is checked *after* the bytes arrive.
    ArchiveUrl(String),
    /// A path inside the archive that is absolute, empty, or contains a `..`
    /// segment — one that could write outside the directory being unpacked into.
    ExecutablePath(String),
}

impl fmt::Display for PinRejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Version(value) => write!(f, "release version cannot name a directory: {value:?}"),
            Self::Platform(value) => write!(
                f,
                "release platform token is not a plain lowercase identifier: {value:?}"
            ),
            Self::Digest(value) => write!(
                f,
                "archive digest is not sixty-four lowercase hex characters: {value:?}"
            ),
            Self::ArchiveUrl(value) => write!(f, "archive must be served over https: {value:?}"),
            Self::ExecutablePath(value) => {
                write!(f, "executable path escapes the archive: {value:?}")
            }
        }
    }
}

impl std::error::Error for PinRejected {}

/// The SHA-256 of an archive, as sixty-four lowercase hexadecimal characters.
///
/// A value object rather than a `String` so that the one comparison that
/// matters — what was pinned against what arrived — cannot be made against an
/// uppercase spelling, a truncation, or a digest of something else that merely
/// happens to be a string. Construction is the only place the shape is checked,
/// so everything downstream holds a digest that is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveDigest(String);

impl ArchiveDigest {
    /// Read a digest from its hexadecimal spelling.
    ///
    /// Deliberately strict about case: accepting both spellings would mean
    /// either normalising here or comparing case-insensitively later, and the
    /// second is the kind of comparison that is easy to write as `==` by
    /// accident. One spelling is allowed, so `==` is always right.
    pub fn parse(value: &str) -> Result<Self, PinRejected> {
        let valid = value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if valid {
            Ok(Self(value.to_owned()))
        } else {
            Err(PinRejected::Digest(value.to_owned()))
        }
    }

    /// The hexadecimal spelling, for a diagnostic or a report.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ArchiveDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The version of an agent's runtime that Nessa has tested against.
///
/// Constrained to what can also be a single directory name, because that is
/// what it becomes: an installed runtime lives under its version, so that
/// changing the pin installs beside the old one rather than half-overwriting
/// it. A version carrying a separator would put it somewhere else entirely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseVersion(String);

impl ReleaseVersion {
    /// Read a version, rejecting anything that could not be a directory name.
    pub fn parse(value: &str) -> Result<Self, PinRejected> {
        let usable = !value.is_empty()
            && value != "."
            && value != ".."
            && !value.contains(['/', '\\'])
            && !value.contains(char::is_whitespace)
            && value.is_ascii();
        if usable {
            Ok(Self(value.to_owned()))
        } else {
            Err(PinRejected::Version(value.to_owned()))
        }
    }

    /// The version as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ReleaseVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The operating system and architecture one archive is built for.
///
/// An agent's runtime is a native binary, so a pin is per-platform and a
/// release is only ever installable on the platform it names. Holding the two
/// tokens together means a lookup cannot accidentally match on one of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleasePlatform {
    operating_system: String,
    architecture: String,
}

impl ReleasePlatform {
    /// Name a platform by the tokens Rust itself uses: `target_os` and
    /// `target_arch`, such as `macos` and `aarch64`.
    pub fn new(operating_system: &str, architecture: &str) -> Result<Self, PinRejected> {
        for token in [operating_system, architecture] {
            let usable = !token.is_empty()
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
            if !usable {
                return Err(PinRejected::Platform(token.to_owned()));
            }
        }
        Ok(Self {
            operating_system: operating_system.to_owned(),
            architecture: architecture.to_owned(),
        })
    }

    pub fn operating_system(&self) -> &str {
        &self.operating_system
    }

    pub fn architecture(&self) -> &str {
        &self.architecture
    }
}

impl fmt::Display for ReleasePlatform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.operating_system, self.architecture)
    }
}

/// A relative path to one file inside an archive.
///
/// The invariant is the reason this is a type: an archive is downloaded from
/// the network and unpacked into a directory Nessa owns, so a path that is
/// absolute or walks upward out of that directory is a way to write anywhere
/// the app can write. Rejecting those shapes at construction means the unpacker
/// cannot be handed one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivePath(String);

impl ArchivePath {
    /// Read a path that stays inside the archive it describes.
    pub fn parse(value: &str) -> Result<Self, PinRejected> {
        let contained = !value.is_empty()
            && !value.starts_with('/')
            && !value.starts_with('\\')
            && !value.contains('\0')
            && value
                .split(['/', '\\'])
                .all(|segment| !segment.is_empty() && segment != "." && segment != "..");
        if contained {
            Ok(Self(value.to_owned()))
        } else {
            Err(PinRejected::ExecutablePath(value.to_owned()))
        }
    }

    /// The path as written, with `/` separators.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ArchivePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why a downloaded archive is not the one that was pinned.
///
/// Its own type rather than a variant of the install failure, because this is
/// the one rejection that says the bytes on the wire were not the bytes Nessa
/// tested — which is a different thing from a network that failed or a disk
/// that was full, and the only one that should never be retried silently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveRejected {
    pub expected: ArchiveDigest,
    pub actual: ArchiveDigest,
}

impl fmt::Display for ArchiveRejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "downloaded archive has digest {} but {} was pinned",
            self.actual, self.expected
        )
    }
}

impl std::error::Error for ArchiveRejected {}

/// One agent runtime release that Nessa has tested and will install.
///
/// This is the whole of what Nessa trusts about a third-party binary: where to
/// get it, what it must hash to, and which file inside it is the executable.
/// Nothing else about the release is taken on faith — in particular the version
/// is not read back out of the downloaded archive, because a tampered archive
/// would be the thing telling us what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedRelease {
    version: ReleaseVersion,
    platform: ReleasePlatform,
    archive_url: String,
    archive_digest: ArchiveDigest,
    executable: ArchivePath,
}

impl PinnedRelease {
    /// Describe a release, refusing any pin that is not self-consistent.
    ///
    /// The URL is required to be `https://` here rather than at the adapter,
    /// because the digest check below happens *after* the bytes have arrived:
    /// plain HTTP would let anyone on the path spend a user's bandwidth and
    /// have the failure look like a corrupted download.
    pub fn new(
        version: ReleaseVersion,
        platform: ReleasePlatform,
        archive_url: &str,
        archive_digest: ArchiveDigest,
        executable: ArchivePath,
    ) -> Result<Self, PinRejected> {
        if !archive_url.starts_with("https://") {
            return Err(PinRejected::ArchiveUrl(archive_url.to_owned()));
        }
        Ok(Self {
            version,
            platform,
            archive_url: archive_url.to_owned(),
            archive_digest,
            executable,
        })
    }

    pub fn version(&self) -> &ReleaseVersion {
        &self.version
    }

    pub fn platform(&self) -> &ReleasePlatform {
        &self.platform
    }

    pub fn archive_url(&self) -> &str {
        &self.archive_url
    }

    pub fn executable(&self) -> &ArchivePath {
        &self.executable
    }

    /// Whether this release is the one to install on `platform`.
    pub fn runs_on(&self, platform: &ReleasePlatform) -> bool {
        &self.platform == platform
    }

    /// Accept or reject what actually arrived.
    ///
    /// The only gate between a downloaded file and an executable Nessa will
    /// launch. It is a method on the release rather than a comparison at the
    /// call site so that there is exactly one of it, and so that the rejection
    /// carries both digests without the caller having to remember to include
    /// them.
    pub fn accept(&self, downloaded: &ArchiveDigest) -> Result<(), ArchiveRejected> {
        if downloaded == &self.archive_digest {
            Ok(())
        } else {
            Err(ArchiveRejected {
                expected: self.archive_digest.clone(),
                actual: downloaded.clone(),
            })
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/agent_install/pinned_release.rs"]
mod tests;
