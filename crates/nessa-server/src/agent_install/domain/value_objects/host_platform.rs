use std::fmt;

use super::pinned_release::{PinRejected, ReleasePlatform};

/// Which C library a build is linked against.
///
/// Only Linux has more than one in practice, and the two are not
/// interchangeable: a binary linked against glibc does not start on a machine
/// that has only musl, and the loader's failure says nothing a user could act
/// on. So this is a fact about the build that has to be matched against the
/// machine, rather than something to discover by launching it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Libc {
    Gnu,
    Musl,
}

impl Libc {
    /// The spelling used in the pin file and in messages.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gnu => "gnu",
            Self::Musl => "musl",
        }
    }

    /// Read one of the two spellings, refusing anything else.
    pub fn parse(value: &str) -> Result<Self, PinRejected> {
        match value {
            "gnu" => Ok(Self::Gnu),
            "musl" => Ok(Self::Musl),
            other => Err(PinRejected::Libc(other.to_owned())),
        }
    }
}

impl fmt::Display for Libc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a machine has to provide for one build to run on it.
///
/// Separate from [`ReleasePlatform`] because several builds share a platform
/// and differ only here. Opencode publishes four Linux x86-64 archives at one
/// version — glibc and musl, each with and without AVX2 — and choosing among
/// them by operating system and architecture alone picks one of the four at
/// random from the machine's point of view. Two of those choices do not start:
/// a glibc build on a musl-only machine, and an AVX2 build on a processor
/// without it, which dies on an illegal instruction rather than saying what is
/// wrong.
///
/// Stated as what the build *needs* rather than as the vendor's name for it.
/// `baseline` and `musl` are Opencode's words for its own artifacts, and a
/// second agent will have different ones; what has to be compared against the
/// machine is the same either way. That separation is what lets the pin file
/// disagree with a name: at 1.18.31 Opencode's three `-baseline` archives hold
/// the same bytes as their AVX2 siblings, so none of them is pinned and a
/// machine without AVX2 matches nothing rather than matching a build named for
/// it. `scripts/agents/pin-opencode.mjs` records the measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReleaseRequirements {
    /// The C library this build needs, where that is a distinction at all.
    ///
    /// `None` means the platform has only one *that Nessa can tell apart*, and
    /// the only platform that is true of is macOS: `target_env` is empty on
    /// every Apple target, so a macOS host has nothing to compare a named C
    /// library against and a macOS release that names one is refused.
    ///
    /// Windows is the open case, however much it looks like macOS here.
    /// `*-pc-windows-gnu` and `*-pc-windows-msvc` are different builds and a
    /// Rust host reports `gnu` or `msvc` for them, so `Some` is a legitimate
    /// thing for a Windows build to say and
    /// [`super::pinned_release::PinnedRelease::new`] accepts it. What a
    /// *vendor's* Windows archive should put here — whether MinGW and MSVC are
    /// a distinction its builds make at all — is for whoever pins the first
    /// Windows release to settle. Writing `null` on the strength of "Windows
    /// is like macOS" would offer one build to both kinds of host, which is
    /// the Linux fault the rule above exists to prevent, with nothing
    /// refusing it.
    libc: Option<Libc>,
    /// Whether the processor must support AVX2. Only ever true for x86-64;
    /// nothing else in the instruction sets Nessa pins for is optional.
    avx2: bool,
}

impl ReleaseRequirements {
    pub fn new(libc: Option<Libc>, avx2: bool) -> Self {
        Self { libc, avx2 }
    }

    pub fn libc(&self) -> Option<Libc> {
        self.libc
    }

    pub fn avx2(&self) -> bool {
        self.avx2
    }

    /// How much this build asks of a machine, as a key two builds can be
    /// ranked by.
    ///
    /// Used to choose between the builds a machine can run, where the vendor's
    /// own default is the demanding one: the undemanding build exists for
    /// machines that cannot take it, and installing it everywhere would give
    /// up what it is there to preserve.
    ///
    /// Ordered on AVX2 first and then on whether a C library is named, which
    /// is a *total* order over any set of builds one machine can run — and
    /// that is what makes the choice independent of the order the pin file
    /// happens to list them in. Two eligible builds that tied here would have
    /// to agree on AVX2 and both name a library or both name none; a machine
    /// satisfies a named library only by having that exact one, so both would
    /// name the same library, and two builds whose requirements agree entirely
    /// are the duplicate the pin reader refuses. A single bit would not be
    /// enough: a build naming no library and one naming this machine's library
    /// are both runnable here, differ, and would tie.
    pub fn demand(&self) -> (bool, bool) {
        (self.avx2, self.libc.is_some())
    }
}

impl fmt::Display for ReleaseRequirements {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.libc, self.avx2) {
            (None, false) => f.write_str("no special requirements"),
            (None, true) => f.write_str("avx2"),
            (Some(libc), false) => write!(f, "{libc}"),
            (Some(libc), true) => write!(f, "{libc} and avx2"),
        }
    }
}

/// One machine, described in the same terms a release states its needs in.
///
/// Built by infrastructure, which is the only thing that can answer what this
/// processor supports and what is installed beside it. Kept in the domain so
/// that choosing a release is a comparison between two values here rather than
/// a condition somebody writes again at each call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPlatform {
    platform: ReleasePlatform,
    libc: Option<Libc>,
    avx2: bool,
}

impl HostPlatform {
    pub fn new(platform: ReleasePlatform, libc: Option<Libc>, avx2: bool) -> Self {
        Self {
            platform,
            libc,
            avx2,
        }
    }

    pub fn platform(&self) -> &ReleasePlatform {
        &self.platform
    }

    /// Whether this machine satisfies everything `requirements` asks for.
    ///
    /// A build that names a C library is refused unless this machine reports
    /// the same one — including when it reports none, which is a machine
    /// nothing could establish this about rather than one that happens to
    /// agree. Guessing in that direction is how a binary that cannot start
    /// gets installed and reported as ready.
    pub fn satisfies(&self, requirements: &ReleaseRequirements) -> bool {
        let libc = match requirements.libc() {
            None => true,
            Some(needed) => self.libc == Some(needed),
        };
        libc && (!requirements.avx2() || self.avx2)
    }
}

impl fmt::Display for HostPlatform {
    /// Says both facts every time, including when the answer is nothing.
    ///
    /// The one place a machine is shown to a person is the message saying
    /// nothing is pinned for it, and the case that needs explaining most is a
    /// C library that could not be established — which, left unsaid, reads as
    /// a platform nobody built for, on a platform pinned four times over.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.platform)?;
        match self.libc {
            Some(libc) => write!(f, " with {libc}")?,
            None => f.write_str(" with no c library nessa could name")?,
        }
        f.write_str(if self.avx2 {
            " and avx2"
        } else {
            " and no avx2"
        })
    }
}

#[cfg(test)]
#[path = "../../../../tests/agent_install/host_platform.rs"]
mod tests;
