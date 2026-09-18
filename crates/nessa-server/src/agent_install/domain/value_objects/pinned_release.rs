use std::fmt;

use url::Url;

/// What a pinned release can be wrong about, at the moment it is described.
///
/// Every variant is a fault in the pin itself — a value checked into this
/// repository — rather than anything a user did. They are separate variants
/// because the pin is also what a future release-bumping script will write, and
/// "the digest is not a digest" and "the executable escapes the archive" are
/// different mistakes to make and different ones to report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinRejected {
    /// A version that cannot also be a directory name everywhere Nessa runs:
    /// empty, too long, carrying something outside the version alphabet,
    /// spelling a name some filesystem reserves, `.` or `..`, or ending in a
    /// dot, which Windows drops rather than stores.
    Version(String),
    /// An operating system or architecture token that is empty or not a plain
    /// lowercase identifier.
    Platform(String),
    /// Not sixty-four lowercase hexadecimal characters.
    Digest(String),
    /// Not a URL at all, or not one an archive may be fetched from: anything
    /// but `https`, a URL with no host, or one carrying credentials. Plain HTTP
    /// would put the archive on the wire for anyone to replace, and the digest
    /// below is checked *after* the bytes arrive.
    ArchiveUrl(String),
    /// A path inside the archive that is not plainly one file below it: empty,
    /// absolute, drive-relative, containing a backslash or a NUL, or having a
    /// segment that is empty, `.` or `..`. Each could name a file outside the
    /// archive or, once joined, outside the directory being unpacked into — or
    /// leave this type and the unpacker disagreeing about where the segments
    /// divide, which comes to the same thing.
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
            Self::ArchiveUrl(value) => {
                write!(f, "archive must be named by an https url: {value:?}")
            }
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

/// The longest a version may be.
///
/// A version becomes one path component, and every filesystem Nessa runs on
/// stops somewhere around 255 bytes. Far below that, because a version longer
/// than this is not a version.
const MAXIMUM_VERSION_LENGTH: usize = 64;

/// Names Windows reserves for devices, which it answers to in any directory and
/// with any extension.
const RESERVED_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// The version of an agent's runtime that Nessa has tested against.
///
/// Constrained to what can also be a single directory name — on every
/// filesystem Nessa runs on, not just this one — because that is what it
/// becomes: an installed runtime lives under its version, so that changing the
/// pin installs beside the old one rather than half-overwriting it.
///
/// Three of the rules below are there for that last sentence rather than for
/// the shell of it. Uppercase is refused because macOS and Windows would give
/// `1.0-Beta` and `1.0-beta` the same directory, and two versions sharing a
/// directory is exactly the half-overwrite this design exists to avoid. A
/// trailing dot and the reserved device names are refused because Windows does
/// not store them as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseVersion(String);

impl ReleaseVersion {
    /// Read a version, rejecting anything that could not be a directory name.
    pub fn parse(value: &str) -> Result<Self, PinRejected> {
        let rejected = || PinRejected::Version(value.to_owned());
        if value.is_empty() || value.len() > MAXIMUM_VERSION_LENGTH {
            return Err(rejected());
        }
        // A closed alphabet rather than a list of things to exclude: it is what
        // a published version is spelled with, and everything a filesystem
        // treats specially — separators, drive colons, control characters,
        // spaces — is outside it without having to be named.
        let spelled = value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'-' | b'+' | b'_')
        });
        let device = value
            .split('.')
            .next()
            .is_some_and(|stem| RESERVED_NAMES.contains(&stem));
        if !spelled || device || value.ends_with('.') || value == "." || value == ".." {
            return Err(rejected());
        }
        Ok(Self(value.to_owned()))
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
    ///
    /// One separator is allowed, `/`, which is the one a tar entry uses. A
    /// backslash is refused rather than treated as a separator: accepting it
    /// would mean every reader of this value had to agree on which characters
    /// divide the segments, and the unpacker, the installed file's name and
    /// this type would each have had to be taught the same rule.
    ///
    /// A colon goes with it. `C:evil` is one legal Unix filename and a
    /// drive-relative path on Windows, where joining it onto a directory
    /// replaces the directory rather than extending it.
    pub fn parse(value: &str) -> Result<Self, PinRejected> {
        let contained = !value.is_empty()
            && !value.starts_with('/')
            && !value.contains(['\\', '\0', ':'])
            && value
                .split('/')
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

    /// The last segment: what the file is called inside the archive.
    ///
    /// Path semantics belong to the value object that holds the invariant, not
    /// to whichever adapter needs the name — and because `parse` refuses an
    /// empty path and empty segments, there is always one to return.
    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }
}

impl fmt::Display for ArchivePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where one release's archive is fetched from.
///
/// Parsed rather than pattern-matched, because the rules that matter here are
/// about the parts of a URL — its scheme, its host, whether it carries
/// credentials — and a prefix test cannot see any of them. `https://` on its own
/// passes `starts_with`, and names nothing.
///
/// Why the scheme is settled here rather than at the adapter: the digest is
/// checked *after* the bytes have arrived, so plain HTTP would let anyone on the
/// path spend a user's bandwidth and have the failure look like a corrupted
/// download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveUrl(Url);

impl ArchiveUrl {
    /// Read a URL an archive may be fetched from.
    pub fn parse(value: &str) -> Result<Self, PinRejected> {
        let rejected = || PinRejected::ArchiveUrl(value.to_owned());
        let parsed = Url::parse(value).map_err(|_| rejected())?;
        // Credentials are refused rather than carried: a pin is a value checked
        // into this repository, and a URL is the wrong place to keep a secret.
        let usable = parsed.scheme() == "https"
            && parsed.host().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none();
        if usable {
            Ok(Self(parsed))
        } else {
            Err(rejected())
        }
    }

    /// The URL as it will be requested.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for ArchiveUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_str())
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
    expected: ArchiveDigest,
    actual: ArchiveDigest,
}

impl ArchiveRejected {
    /// What the pin says the archive hashes to.
    pub fn expected(&self) -> &ArchiveDigest {
        &self.expected
    }

    /// What the downloaded bytes actually hashed to.
    pub fn actual(&self) -> &ArchiveDigest {
        &self.actual
    }
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
    archive_url: ArchiveUrl,
    archive_digest: ArchiveDigest,
    executable: ArchivePath,
}

impl PinnedRelease {
    /// Describe a release.
    ///
    /// Infallible: every part of a pin that can be wrong is wrong at the moment
    /// that part is read, and each one is its own type above. Assembling four
    /// values that are each already valid cannot produce an invalid release, so
    /// there is nothing left here to refuse.
    pub fn new(
        version: ReleaseVersion,
        platform: ReleasePlatform,
        archive_url: ArchiveUrl,
        archive_digest: ArchiveDigest,
        executable: ArchivePath,
    ) -> Self {
        Self {
            version,
            platform,
            archive_url,
            archive_digest,
            executable,
        }
    }

    pub fn version(&self) -> &ReleaseVersion {
        &self.version
    }

    pub fn platform(&self) -> &ReleasePlatform {
        &self.platform
    }

    pub fn archive_url(&self) -> &ArchiveUrl {
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
