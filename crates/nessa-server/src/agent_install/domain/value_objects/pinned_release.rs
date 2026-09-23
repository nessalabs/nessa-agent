use std::fmt;

use url::Url;

use super::device_names::names_a_device;
use super::host_platform::{HostPlatform, ReleaseRequirements};
use super::release_contents::{ArchivePath, ReleaseContents};

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
    /// A C library the pin names that this build has no notion of. Left as a
    /// refusal rather than treated as "no requirement", because a pin naming
    /// one Nessa cannot check is a pin whose build might not start.
    Libc(String),
    /// Not sixty-four lowercase hexadecimal characters.
    Digest(String),
    /// A length no published archive has: nothing at all, or more than any
    /// agent runtime is. The pin states the archive's exact size and the fetch
    /// is held to it, so a number that is wrong in the generous direction is a
    /// pin handing a stranger permission to fill a disk.
    ArchiveSize(u64),
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
    FilePath(String),
    /// The set of files a release installs is not one a release could have:
    /// empty, naming no program to launch or naming two, naming one path
    /// twice, or naming a path that would have to be a file and a directory at
    /// once. Its own variant because every one of them spans several paths,
    /// and none is a fault of any single one of them.
    Contents(String),
    /// What the build needs and the platform it is for do not agree. Every
    /// other variant above is about one value being unreadable on its own;
    /// this is the one rule that spans two of them, which is why it is refused
    /// here rather than left to whoever assembles the pair.
    Requirements(String),
}

impl fmt::Display for PinRejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Version(value) => write!(f, "release version cannot name a directory: {value:?}"),
            Self::Platform(value) => write!(
                f,
                "release platform token is not a plain lowercase identifier: {value:?}"
            ),
            Self::Libc(value) => {
                write!(f, "release names a c library nessa cannot check: {value:?}")
            }
            Self::Requirements(detail) => {
                write!(f, "release requirements do not fit its platform: {detail}")
            }
            Self::Digest(value) => write!(
                f,
                "archive digest is not sixty-four lowercase hex characters: {value:?}"
            ),
            Self::ArchiveSize(value) => write!(
                f,
                "archive size is not a length a published archive has: {value} bytes"
            ),
            Self::ArchiveUrl(value) => {
                write!(f, "archive must be named by an https url: {value:?}")
            }
            Self::FilePath(value) => {
                write!(f, "release file path escapes the archive: {value:?}")
            }
            Self::Contents(detail) => {
                write!(f, "release does not describe what it installs: {detail}")
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

/// The most bytes a pinned archive may be said to be.
///
/// A ceiling on what a *pin* may claim, not on what a server may send — the
/// fetch is held to the exact size below. It is here because the fetch is held
/// to that number: a pin that overstated it by a factor of a hundred would be
/// this repository granting whoever answers the request permission to write a
/// hundred times an agent runtime onto somebody's disk. Around double the
/// largest archive Nessa pins, which is Codex at 117 MB compressed.
const MAXIMUM_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;

/// How many bytes one release's archive is, exactly.
///
/// Measured when the release was pinned, from the same download the digest was
/// taken from, and it is what the fetch is bounded by. The digest alone would
/// catch a body that was not the pinned archive — but only after all of it had
/// been written to the disk, so an endless answer to a hundred-megabyte request
/// would be a full disk reported as a failed download. A pinned length turns
/// that into a refusal at the first byte past the archive.
///
/// A value object rather than a bare `u64` so the two rules that make it usable
/// as a bound — it is a real length, and it is not an absurd one — are checked
/// where the number is read rather than remembered at each call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveSize(u64);

impl ArchiveSize {
    /// Read a pinned archive length, refusing one no archive has.
    pub fn parse(value: u64) -> Result<Self, PinRejected> {
        if value == 0 || value > MAXIMUM_ARCHIVE_BYTES {
            return Err(PinRejected::ArchiveSize(value));
        }
        Ok(Self(value))
    }

    /// The length, for the fetch that is bounded by it.
    pub fn bytes(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ArchiveSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} bytes", self.0)
    }
}

/// The longest a version may be.
///
/// A version becomes one path component, and every filesystem Nessa runs on
/// stops somewhere around 255 bytes. Far below that, because a version longer
/// than this is not a version.
const MAXIMUM_VERSION_LENGTH: usize = 64;

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
        if !spelled
            || names_a_device(value)
            || value.ends_with('.')
            || value == "."
            || value == ".."
        {
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
/// get it, how long it is, what it must hash to, and which files inside it are
/// installed. Nothing else about the release is taken on faith — in particular
/// the version is not read back out of the downloaded archive, because a
/// tampered archive would be the thing telling us what it is, and neither is
/// the list of files, for the same reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedRelease {
    version: ReleaseVersion,
    platform: ReleasePlatform,
    requirements: ReleaseRequirements,
    archive_url: ArchiveUrl,
    archive_size: ArchiveSize,
    archive_digest: ArchiveDigest,
    contents: ReleaseContents,
}

impl PinnedRelease {
    /// Describe a release.
    ///
    /// Almost every part of a pin that can be wrong is wrong at the moment that
    /// part is read, and each one is its own type above. What is left, and what
    /// this refuses, is the pair: [`ReleaseRequirements`] and
    /// [`ReleasePlatform`] are each perfectly valid alone and can still
    /// describe a build that cannot exist.
    ///
    /// Three such rules, and each decides whether a machine is offered
    /// something it cannot start, which is what this whole context exists to
    /// prevent:
    ///
    /// - A Linux build names a C library. Every Linux build is linked against
    ///   one, so `None` there is a field somebody left out rather than a build
    ///   that runs against either — and [`HostPlatform::satisfies`] reads an
    ///   unnamed library as "nothing required", so such a release would be
    ///   offered to every Linux machine and would fail in the loader on half
    ///   of them.
    /// - A macOS build names none. This is the mirror of the rule above and
    ///   fails the other way: no Apple target is built against musl or glibc,
    ///   so `host_libc` answers `None` on every macOS machine there is, and a
    ///   macOS release naming a library is one nothing can satisfy. It would
    ///   install nowhere and say nothing about why.
    ///
    ///   Windows is deliberately not included. A `*-windows-gnu` build of
    ///   Nessa does report `gnu` — MinGW, a different fact wearing the same
    ///   name — so a Windows release naming it is one some build could
    ///   actually match. Refusing it here would reject a pin nobody has
    ///   written yet on a guess about what the word will mean when they do.
    /// - AVX2 is asked for only where a processor could have it. An `aarch64`
    ///   build requiring it is not dangerous, only unreachable: nothing would
    ///   ever satisfy it, so the pin would quietly install on no machine at
    ///   all. A requirement nothing can meet is a typo, not a requirement.
    ///
    /// Here rather than in the adapter that reads the pin file, because a pin
    /// file is one way to assemble a release and not the only one: a second
    /// source, or a caller in a test, would otherwise construct exactly the
    /// release these two rules exist to refuse.
    pub fn new(
        version: ReleaseVersion,
        platform: ReleasePlatform,
        requirements: ReleaseRequirements,
        archive_url: ArchiveUrl,
        archive_size: ArchiveSize,
        archive_digest: ArchiveDigest,
        contents: ReleaseContents,
    ) -> Result<Self, PinRejected> {
        if platform.operating_system() == "linux" && requirements.libc().is_none() {
            return Err(PinRejected::Requirements(format!(
                "{platform} does not say which c library it needs"
            )));
        }
        if platform.operating_system() == "macos" && requirements.libc().is_some() {
            return Err(PinRejected::Requirements(format!(
                "{platform} names a c library, which no macos build of nessa reports"
            )));
        }
        // `x86` as well as `x86_64`: a 32-bit x86 processor can have AVX2, and
        // the host answer is read from the processor rather than the target.
        if requirements.avx2() && !matches!(platform.architecture(), "x86_64" | "x86") {
            return Err(PinRejected::Requirements(format!(
                "{platform} requires avx2, which no {} processor has",
                platform.architecture()
            )));
        }
        Ok(Self {
            version,
            platform,
            requirements,
            archive_url,
            archive_size,
            archive_digest,
            contents,
        })
    }

    pub fn version(&self) -> &ReleaseVersion {
        &self.version
    }

    pub fn platform(&self) -> &ReleasePlatform {
        &self.platform
    }

    pub fn requirements(&self) -> &ReleaseRequirements {
        &self.requirements
    }

    pub fn archive_digest(&self) -> &ArchiveDigest {
        &self.archive_digest
    }

    pub fn archive_size(&self) -> ArchiveSize {
        self.archive_size
    }

    pub fn archive_url(&self) -> &ArchiveUrl {
        &self.archive_url
    }

    /// Every file this release installs, and which of them is launched.
    pub fn contents(&self) -> &ReleaseContents {
        &self.contents
    }

    /// The program this release is launched by, which is the one thing outside
    /// this module ever asks a release about a single file.
    pub fn launch(&self) -> &ArchivePath {
        self.contents.launch()
    }

    /// Whether this release will run on `host`.
    ///
    /// Two questions, not one. The platform has to be the same, and the machine
    /// has to provide what the build needs — which for one platform can differ
    /// from one pinned archive to the next. Answering only the first is how a
    /// machine is handed a binary that matches its operating system and
    /// architecture and still cannot start.
    pub fn runs_on(&self, host: &HostPlatform) -> bool {
        &self.platform == host.platform() && host.satisfies(&self.requirements)
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
