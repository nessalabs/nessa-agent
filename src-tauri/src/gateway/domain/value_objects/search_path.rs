use std::{error::Error, fmt};
// Named only by `excluding`, which is gated with the host that has a directory
// to exclude.
#[cfg(target_os = "macos")]
use std::path::Path;

/// A `PATH` value: colon-separated absolute directories, in the order they are
/// searched.
///
/// Two processes are given one of these. The gateway service gets
/// [`SearchPath::system`], because everything it runs it addresses by absolute
/// path. The agent gets one built from the user's login shell, because the
/// point of the agent is to run the tools that user installed.
///
/// A login shell is user-controlled code, so its output arrives here as an
/// untrusted string: [`SearchPath::parse`] is the only way in, and
/// what it accepts is stated in its own documentation. There is no mutation —
/// [`SearchPath::excluding`] answers with a replacement value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPath(String);

/// Why a candidate string is not a search path.
///
/// Each variant is a distinct reason, because "the login shell said nothing" and
/// "the login shell said something unusable" are different things to report.
// Constructed where a reported path is parsed, which is where a login shell can
// be read; see the note on `impl SearchPath`.
#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchPathError {
    /// Longer than [`SearchPath::LIMIT`] bytes. A real `PATH` is not, and an
    /// unbounded one is a login shell that printed something else entirely.
    TooLong,
    /// Contains a NUL, a newline, or another control character: not a `PATH`,
    /// whatever else it is.
    Control,
    /// Held no absolute directory at all. Empty and relative entries are
    /// dropped, so a value that consisted only of those is left with nothing.
    NoAbsoluteEntry,
}
impl fmt::Display for SearchPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TooLong => "search path exceeds its length limit",
            Self::Control => "search path contains control characters",
            Self::NoAbsoluteEntry => "search path has no absolute entry",
        })
    }
}
impl Error for SearchPathError {}

// Every target carries this value: `GatewayHost::register` and `LoginShellPath`
// both name it, and those contracts read the same everywhere. Building one is
// what a host that can ask a login shell for a `PATH` does, and that is a Unix
// host — a Windows build compiles the value and reaches none of it, the way it
// compiles `ReconciledGateway` with no launchd to fill one in. The rules are
// still exercised on every platform by this module's own tests.
#[cfg_attr(not(unix), allow(dead_code))]
impl SearchPath {
    /// The minimal set of system directories every Nessa process can rely on.
    ///
    /// It is what launchd would give a service with no `PATH` of its own, and
    /// it is the floor the agent falls back to when the login shell cannot be
    /// read.
    const SYSTEM_ENTRIES: &'static str = "/usr/bin:/bin:/usr/sbin:/sbin";

    /// The most a `PATH` may be. Chosen well above any real one — a long
    /// developer `PATH` is a few hundred bytes — and well below the point where
    /// a runaway shell could make the service definition unreadable.
    pub const LIMIT: usize = 4096;

    /// The system directories, and nothing else.
    pub fn system() -> Self {
        Self(Self::SYSTEM_ENTRIES.into())
    }

    /// A reported `PATH` — what a login shell printed, or what an already
    /// registered service definition says it was given — as a value this host
    /// will hand to a child process.
    ///
    /// The string is untrusted in both cases: one is the output of the user's
    /// own profile, the other a file on disk that anything with the user's
    /// privileges could have written. What survives:
    ///
    /// - nothing longer than [`SearchPath::LIMIT`] bytes, and nothing holding a
    ///   control character — a shell that printed a banner, a prompt or a NUL
    ///   is rejected whole rather than half-read;
    /// - absolute entries only, in their original order. An empty entry means
    ///   "the working directory" to every shell that reads a `PATH`, and a
    ///   relative one means whatever directory a tool happened to be run from;
    ///   neither is something to hand an agent, so both are dropped.
    ///
    /// Returns [`SearchPathError::NoAbsoluteEntry`] if that leaves nothing,
    /// rather than an empty path that would silently search the working
    /// directory.
    pub fn parse(reported: &str) -> Result<Self, SearchPathError> {
        if reported.len() > Self::LIMIT {
            return Err(SearchPathError::TooLong);
        }
        if reported.chars().any(char::is_control) {
            return Err(SearchPathError::Control);
        }
        Self::from_entries(reported.split(':')).ok_or(SearchPathError::NoAbsoluteEntry)
    }

    /// The same path without `directory`, or `None` if it was the only entry.
    ///
    /// The staged runtime is what this exists for: it holds Nessa's own `node`,
    /// `nessa` and `nessa-mcp`, which the gateway addresses by absolute path and
    /// the agent must not reach by name. A user whose login shell somehow names
    /// that directory still does not get a `node` that shadows their project's.
    ///
    /// Gated like the only thing that stages a runtime and registers a service
    /// to run it: the launchd adapter. A host that has no such directory has
    /// nothing to take out, and the gate moves when a second host grows one.
    #[cfg(target_os = "macos")]
    pub fn excluding(&self, directory: &Path) -> Option<Self> {
        // A directory this host cannot spell in UTF-8 is not an entry of a path
        // that is one, so there is nothing to take out.
        let Some(directory) = directory.to_str() else {
            return Some(self.clone());
        };
        Self::from_entries(self.0.split(':').filter(|entry| *entry != directory))
    }

    /// The colon-separated form, for the child's environment.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Absolute is spelled out rather than asked of `std::path::Path`: what
    /// counts as an absolute path differs by host, and what counts as an entry
    /// of a `PATH` the agent will be given must not.
    fn from_entries<'a>(entries: impl Iterator<Item = &'a str>) -> Option<Self> {
        let kept: Vec<&str> = entries.filter(|entry| entry.starts_with('/')).collect();
        (!kept.is_empty()).then(|| Self(kept.join(":")))
    }
}

#[cfg(test)]
#[path = "../../../../tests/gateway/domain/search_path.rs"]
mod tests;
