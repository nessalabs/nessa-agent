//! Where this build lives on a host, and the commands a gateway runs there
//! over SSH to find it, put it there, and start it (issue #703).
//!
//! ```text
//! serve_command   ──▶ exec ~/.nessa/env/<digest>/nessa env serve
//!                     absent: the stream ends with no hello
//! probe_command   ──▶ "present", or "absent <os> <arch> <libc>"
//! upload_command  ──▶ stdin to a private temporary file under ~/.nessa/env
//!                 ──▶ its SHA-256 is the digest sent?      no ──▶ "refused fingerprint <seen>"
//!                 ──▶ it runs, and `env protocol` says
//!                     this build's protocol?             no ──▶ "refused version <seen>"
//!                 ──▶ renamed into ~/.nessa/env/<digest>/nessa ──▶ "installed"
//! ```
//!
//! Arrows are what each command does, in order, and what it prints. The
//! directory is named by the build's own SHA-256, so a gateway runs only the
//! exact bytes it is: a build whose lease protocol is unchanged but whose
//! harness or dependencies are not (the host runs its harness too) is
//! absent until it is installed, a host keeps one copy per build, and two
//! gateways of different builds never replace each other's. The lease
//! protocol is still checked, on the copy itself, before it is placed and
//! when it is probed. Nothing is
//! renamed into place before it verified, and the rename is the only step
//! that publishes, so a copy cut short or refused is never found.
//!
//! Every command is `sh -c '<script>'`: `sshd` hands the command to the
//! account's login shell, and bash, zsh, fish and csh all pass a
//! single-quoted word on unread. So a script holds no single quote, `!`,
//! backslash or newline (`every_command_survives_any_login_shell`), and what
//! varies in it — the protocol, the digest — is hex.
//!
//! Every build of one protocol on a host shares that host's data directory,
//! and so its one serving lock and its lease ledger: the ledger is written
//! by `env_serve`, all of whose sources the protocol names, so builds of one
//! protocol read it alike, and one still serving holds the others off as
//! `environment_busy`.
//!
//! The harness this build starts, and the account and credentials it runs
//! with, are the host's own (`config.json` there, the SSH account): nothing
//! here installs an agent or creates an account.
use std::fmt;

/// Where builds are kept, under the account's home directory.
pub const INSTALL_DIRECTORY: &str = ".nessa/env";

/// How long a temporary file of an install that never finished is kept
/// before a later install removes it, in minutes.
const ABANDONED_MINUTES: u32 = 60;

/// The command that starts the build whose SHA-256 is `build` in its
/// environment role on a host.
#[must_use]
pub fn serve_command(build: &str) -> String {
    hex(build);
    format!("sh -c 'exec \"$HOME/{INSTALL_DIRECTORY}/{build}/nessa\" env serve'")
}

/// The command that says whether the build whose SHA-256 is `build` is
/// installed on a host — a copy where it is kept that runs and says it
/// speaks `protocol` — and if not, what the host runs on. A copy that no longer runs (the host's C library changed, its
/// home is now mounted without execution) is absent, so it is installed
/// again or refused with the reason.
#[must_use]
pub fn probe_command(protocol: &str, build: &str) -> String {
    hex(protocol);
    hex(build);
    format!(
        "sh -c '\
p=\"$HOME/{INSTALL_DIRECTORY}/{build}/nessa\"; \
if [ -x \"$p\" ] && [ \"$(\"$p\" env protocol 2>/dev/null)\" = \"{protocol}\" ]; then echo present; else \
if getconf GNU_LIBC_VERSION >/dev/null 2>&1; then l=gnu; else l=other; fi; \
echo \"absent $(uname -s) $(uname -m) $l\"; fi'"
    )
}

/// The command that installs the build whose bytes follow on its standard
/// input where it is kept by `digest`, refusing them unless their SHA-256 is
/// `digest` and, once run, they say they speak `protocol`.
#[must_use]
pub fn upload_command(protocol: &str, digest: &str) -> String {
    hex(protocol);
    hex(digest);
    let refuse = |what: &str| format!("{{ rm -f \"$t\"; echo \"{what}\"; exit 0; }}");
    format!(
        "sh -c '\
umask 077; d=\"$HOME/{INSTALL_DIRECTORY}\"; \
mkdir -p \"$d\" || {{ echo \"failed directory\"; exit 0; }}; \
find \"$d\" -maxdepth 1 -name \".install.*\" -mmin +{ABANDONED_MINUTES} -exec rm -f {{}} + 2>/dev/null; \
t=$(mktemp \"$d/.install.XXXXXX\") || {{ echo \"failed temporary\"; exit 0; }}; \
cat > \"$t\" || {failed_write}; \
if command -v sha256sum >/dev/null 2>&1; then s=$(sha256sum < \"$t\"); \
elif command -v shasum >/dev/null 2>&1; then s=$(shasum -a 256 < \"$t\"); \
else {no_digest}; fi; \
set -- $s; [ \"$1\" = \"{digest}\" ] || {fingerprint}; \
chmod 700 \"$t\" || {failed_mode}; \
v=$(\"$t\" env protocol 2>/dev/null) || {unrunnable}; \
[ \"$v\" = \"{protocol}\" ] || {version}; \
mkdir -p \"$d/{digest}\" && mv -f \"$t\" \"$d/{digest}/nessa\" || {failed_publish}; \
echo installed'",
        failed_write = refuse("failed write"),
        no_digest = refuse("refused digest_tool"),
        fingerprint = refuse("refused fingerprint $1"),
        failed_mode = refuse("failed mode"),
        unrunnable = refuse("refused unrunnable"),
        version = refuse("refused version $v"),
        failed_publish = refuse("failed publish"),
    )
}

/// A protocol or digest is lowercase hex, so it can sit in a script.
fn hex(value: &str) {
    assert!(
        !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "only hex goes into a host command"
    );
}

/// An operating system, processor and C library, as this build was made for
/// or as a host's probe named them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Platform {
    /// `macos`, `linux`, or what `uname -s` said, lowercased.
    pub os: String,
    /// `aarch64`, `x86_64`, or what `uname -m` said.
    pub arch: String,
    /// On Linux, `gnu` when the GNU C library is there; otherwise `other`.
    pub libc: String,
}

impl Platform {
    /// What this build runs on.
    #[must_use]
    pub fn this_build() -> Self {
        let libc = if cfg!(target_env = "gnu") {
            "gnu"
        } else {
            "other"
        };
        Self {
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            libc: libc.into(),
        }
    }

    /// A host's platform from its probe's words.
    fn from_probe(os: &str, arch: &str, libc: &str) -> Self {
        let os = match os {
            "Darwin" => "macos".to_owned(),
            other => other.to_ascii_lowercase(),
        };
        let arch = match arch {
            "arm64" => "aarch64",
            "amd64" => "x86_64",
            other => other,
        };
        Self {
            os,
            arch: arch.into(),
            libc: libc.into(),
        }
    }

    /// Whether a build made for `self` runs on `host`: the same system and
    /// processor, and on Linux a GNU build only where the GNU C library is.
    /// A macOS host's C library is the system's, whatever its probe said.
    #[must_use]
    pub fn runs_on(&self, host: &Self) -> bool {
        self.os == host.os
            && self.arch == host.arch
            && (self.os != "linux" || self.libc != "gnu" || host.libc == "gnu")
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {} {}", self.os, self.arch, self.libc)
    }
}

/// What a host's probe said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    /// This build is installed there.
    Present,
    /// It is not, and the host runs on this.
    Absent(Platform),
}

impl Probe {
    /// The probe's answer, from the last line of its output (a login
    /// shell's own greeting may come before it); `None` for anything else.
    #[must_use]
    pub fn parse(output: &str) -> Option<Self> {
        let line = output.lines().rev().find(|line| !line.trim().is_empty())?;
        match line.split_whitespace().collect::<Vec<_>>()[..] {
            ["present"] => Some(Self::Present),
            ["absent", os, arch, libc] => Some(Self::Absent(Platform::from_probe(os, arch, libc))),
            _ => None,
        }
    }
}

/// What an upload said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Upload {
    /// Verified and renamed into place.
    Installed,
    /// Not installed, and why; nothing of it is left where it would be found.
    Refused(UploadRefusal),
}

/// Why an upload installed nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UploadRefusal {
    /// What arrived is not what was sent: its SHA-256, as the host saw it.
    Fingerprint(String),
    /// What arrived runs, and speaks another lease protocol: what it said.
    Version(String),
    /// What arrived does not run there (a C library too old, a home
    /// directory mounted without execution).
    Unrunnable,
    /// The host has neither `sha256sum` nor `shasum`, so nothing can be
    /// verified there.
    NoDigestTool,
    /// A step on the host failed: making the directory or the temporary
    /// file, writing, setting its mode, or renaming it into place.
    Failed(String),
}

impl Upload {
    /// The upload's answer, from the last line of its output; `None` for
    /// anything else. What the host saw is kept to 128 printable characters.
    #[must_use]
    pub fn parse(output: &str) -> Option<Self> {
        let line = output.lines().rev().find(|line| !line.trim().is_empty())?;
        let seen = |words: &[&str]| -> String {
            words
                .join(" ")
                .chars()
                .filter(|character| character.is_ascii_graphic() || *character == ' ')
                .take(128)
                .collect()
        };
        let words: Vec<&str> = line.split_whitespace().collect();
        match words[..] {
            ["installed"] => Some(Self::Installed),
            ["refused", "fingerprint", ref rest @ ..] => {
                Some(Self::Refused(UploadRefusal::Fingerprint(seen(rest))))
            }
            ["refused", "version", ref rest @ ..] => {
                Some(Self::Refused(UploadRefusal::Version(seen(rest))))
            }
            ["refused", "unrunnable"] => Some(Self::Refused(UploadRefusal::Unrunnable)),
            ["refused", "digest_tool"] => Some(Self::Refused(UploadRefusal::NoDigestTool)),
            ["failed", step] => Some(Self::Refused(UploadRefusal::Failed(seen(&[step])))),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "../../tests/env_serve/install.rs"]
mod tests;
