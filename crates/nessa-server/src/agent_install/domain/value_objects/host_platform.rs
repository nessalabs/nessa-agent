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
/// machine is the same either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReleaseRequirements {
    /// The C library this build needs, where that is a distinction at all.
    /// `None` means the platform has only one, as macOS and Windows do.
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
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.platform)?;
        if let Some(libc) = self.libc {
            write!(f, " with {libc}")?;
        }
        if self.avx2 {
            f.write_str(" and avx2")?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../tests/agent_install/host_platform.rs"]
mod tests;
