//! First use of a host (issue #703): when a host's stream ends with nothing
//! said on it, the environment asks whether this build is installed there,
//! and if it is not, puts it there before connecting again.
//!
//! ```text
//! ensure(host)
//!   ──▶ probe ──▶ present ──▶ Present: nothing written (the host's own trouble)
//!             ──▶ no answer ──▶ refused environment_unreachable
//!             ──▶ absent <platform> ──▶ this build runs there?
//!                    no ──▶ InstallRefused{platform} ──▶ environment_platform_unsupported
//!   ──▶ this build's executable, opened, and its SHA-256
//!   ──▶ InstallStarted recorded (or nothing is sent)
//!   ──▶ upload ──▶ installed ──▶ Installed recorded ──▶ Installed
//!                                 (unrecorded ──▶ environment_install_failed)
//!              ──▶ refused fingerprint | unrunnable | digest tool | failed
//!                     ──▶ InstallRefused ──▶ environment_install_failed
//!              ──▶ refused version ──▶ InstallRefused ──▶ environment_version_mismatch
//!              ──▶ no answer ──▶ probe again: present ──▶ InstallFound ──▶ Installed
//!                                           (unrecorded ──▶ environment_install_failed)
//!                                           otherwise ──▶ InstallRefused{unanswered}
//!                                                         ──▶ environment_unreachable
//! ```
//!
//! Arrows are steps, in order. What is installed is this gateway's own
//! executable, and only where it runs: the same system and processor (and
//! on Linux the GNU C library, which it links). That is the one copy that is
//! certain to speak this build's lease protocol; no other platform's build
//! of this exact source is published anywhere a digest could pin. The
//! host's verdicts — the digest, then the protocol the copy itself states —
//! are taken there before the copy is put where it is found
//! (`env_serve::install`), and the hello after it is checked as any other.
//!
//! An install whose outcome cannot be recorded is not used, as a connection
//! that cannot be recorded is not: the lease is refused. The copy stays on
//! the host, verified, so the next lease is served by it and recorded
//! `Connected`; the audit keeps the `InstallStarted` with no outcome.
use super::audit::{EnvironmentAudit, EnvironmentEvent, InstallRefusal};
use crate::env::LEASE_PROTOCOL;
use crate::env_serve::install::{
    probe_command, upload_command, Platform, Probe, Upload, UploadRefusal,
};
use nessa_sdk::domain::agent_execution::leases::{LeaseRefusal, SshDestination};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    future::Future,
    io::{self, Seek, SeekFrom},
    path::Path,
    pin::Pin,
    sync::Arc,
    time::Duration,
};

/// What a command run on a host printed.
pub(crate) type ShellFuture<'a> = Pin<Box<dyn Future<Output = io::Result<String>> + Send + 'a>>;

/// Runs one command on a host and answers what it printed.
pub(crate) trait RemoteShell: Send + Sync {
    /// Run `command` on `host`, with `input` as its standard input (none:
    /// an empty one), and answer the start of what it printed.
    ///
    /// # Errors
    /// The command could not be run, or its output read.
    fn run<'a>(
        &'a self,
        host: &'a SshDestination,
        command: String,
        input: Option<File>,
    ) -> ShellFuture<'a>;
}

/// A copy of this build to send: opened, at its start, and its SHA-256.
pub(crate) struct Build {
    pub(crate) file: File,
    pub(crate) digest: String,
}

/// Where the copy of this build sent to a host comes from.
pub(crate) trait BuildSource: Send + Sync {
    /// Open it. The digest is of the bytes the file reads from its start,
    /// so what is sent is what was measured, even if the path is replaced
    /// meanwhile.
    ///
    /// # Errors
    /// It could not be read.
    fn open(&self) -> io::Result<Build>;
}

/// This process's own executable.
pub(crate) struct OwnExecutable;

impl BuildSource for OwnExecutable {
    fn open(&self) -> io::Result<Build> {
        // On Linux the running file itself, even once its path was replaced
        // by an update or removed: `current_exe` would name whatever is at
        // the path now.
        if cfg!(target_os = "linux") {
            return open_build(Path::new("/proc/self/exe"));
        }
        open_build(&std::env::current_exe()?)
    }
}

/// The build at `path`, opened, and its SHA-256.
///
/// # Errors
/// It could not be read.
pub(crate) fn open_build(path: &Path) -> io::Result<Build> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher)?;
    file.seek(SeekFrom::Start(0))?;
    Ok(Build {
        file,
        digest: format!("{:x}", hasher.finalize()),
    })
}

/// How long a host's answers may take.
#[derive(Clone, Copy, Debug)]
pub(crate) struct InstallTimings {
    /// The probe's answer.
    pub(crate) probe: Duration,
    /// The upload: sending this build, verifying and placing it.
    pub(crate) upload: Duration,
}

impl Default for InstallTimings {
    fn default() -> Self {
        Self {
            probe: Duration::from_secs(30),
            upload: Duration::from_secs(600),
        }
    }
}

/// What a host had, or has now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Installation {
    /// This build was already installed; nothing was written.
    Present,
    /// It was not, and now is.
    Installed,
}

/// Installs this build on hosts that have none.
pub(crate) struct HostInstaller {
    shell: Arc<dyn RemoteShell>,
    build: Arc<dyn BuildSource>,
    platform: Platform,
    protocol: &'static str,
    timings: InstallTimings,
}

impl HostInstaller {
    pub(crate) fn new(
        shell: Arc<dyn RemoteShell>,
        build: Arc<dyn BuildSource>,
        platform: Platform,
        protocol: &'static str,
        timings: InstallTimings,
    ) -> Self {
        Self {
            shell,
            build,
            platform,
            protocol,
            timings,
        }
    }

    /// This build, its platform and protocol, sent by `shell`.
    pub(crate) fn this_build(shell: Arc<dyn RemoteShell>) -> Self {
        Self::new(
            shell,
            Arc::new(OwnExecutable),
            Platform::this_build(),
            LEASE_PROTOCOL,
            InstallTimings::default(),
        )
    }

    /// Make sure this build is installed on `host`.
    ///
    /// # Errors
    /// The typed refusal; each install refused is recorded in `audit`.
    pub(crate) async fn ensure(
        &self,
        host: &SshDestination,
        audit: &dyn EnvironmentAudit,
    ) -> Result<Installation, LeaseRefusal> {
        let name = host.as_str();
        let platform = match self.probe(host).await {
            Some(Probe::Present) => return Ok(Installation::Present),
            Some(Probe::Absent(platform)) => platform,
            None => return Err(LeaseRefusal::EnvironmentUnreachable),
        };
        if !self.platform.runs_on(&platform) {
            refused(
                audit,
                name,
                InstallRefusal::Platform,
                Some(platform.to_string()),
            );
            return Err(LeaseRefusal::EnvironmentPlatformUnsupported);
        }
        let build = self.build.clone();
        let opened = tokio::task::spawn_blocking(move || build.open())
            .await
            .map_err(io::Error::other)
            .and_then(|opened| opened);
        let Build { file, digest } = match opened {
            Ok(build) => build,
            Err(error) => {
                tracing::error!(%error, "this build's executable could not be read to install it");
                refused(audit, name, InstallRefusal::Source, None);
                return Err(LeaseRefusal::EnvironmentInstallFailed);
            }
        };
        let started = EnvironmentEvent::InstallStarted {
            host: name.into(),
            protocol: self.protocol.into(),
            digest: digest.clone(),
        };
        if let Err(error) = audit.record(&started) {
            tracing::error!(%error, "an install could not be recorded; nothing is sent");
            return Err(LeaseRefusal::EnvironmentInstallFailed);
        }
        let uploaded = tokio::time::timeout(
            self.timings.upload,
            self.shell
                .run(host, upload_command(self.protocol, &digest), Some(file)),
        )
        .await;
        let answer = match uploaded {
            Ok(Ok(output)) => Upload::parse(&output),
            Ok(Err(error)) => {
                tracing::warn!(host = name, %error, "the upload could not be run");
                None
            }
            Err(_) => {
                tracing::warn!(host = name, "the upload did not answer in time");
                None
            }
        };
        // An answer lost after the host put the copy in place (the
        // connection dropped, or the deadline passed, between its rename
        // and its word) is asked of the host again rather than recorded as
        // refused: a copy the probe finds there runs and speaks this
        // protocol.
        if answer.is_none() && self.probe(host).await == Some(Probe::Present) {
            let found = EnvironmentEvent::InstallFound {
                host: name.into(),
                protocol: self.protocol.into(),
            };
            if let Err(error) = audit.record(&found) {
                tracing::error!(%error, "a copy found after a lost answer could not be recorded; it is not used");
                return Err(LeaseRefusal::EnvironmentInstallFailed);
            }
            return Ok(Installation::Installed);
        }
        let (reason, seen, refusal) = match answer {
            Some(Upload::Installed) => {
                let installed = EnvironmentEvent::Installed {
                    host: name.into(),
                    protocol: self.protocol.into(),
                    digest,
                };
                if let Err(error) = audit.record(&installed) {
                    tracing::error!(%error, "a finished install could not be recorded; it is not used");
                    return Err(LeaseRefusal::EnvironmentInstallFailed);
                }
                return Ok(Installation::Installed);
            }
            Some(Upload::Refused(UploadRefusal::Fingerprint(seen))) => (
                InstallRefusal::Fingerprint,
                Some(seen),
                LeaseRefusal::EnvironmentInstallFailed,
            ),
            Some(Upload::Refused(UploadRefusal::Version(seen))) => (
                InstallRefusal::Version,
                Some(seen),
                LeaseRefusal::EnvironmentVersionMismatch,
            ),
            Some(Upload::Refused(UploadRefusal::Unrunnable)) => (
                InstallRefusal::Unrunnable,
                None,
                LeaseRefusal::EnvironmentInstallFailed,
            ),
            Some(Upload::Refused(UploadRefusal::NoDigestTool)) => (
                InstallRefusal::DigestTool,
                None,
                LeaseRefusal::EnvironmentInstallFailed,
            ),
            Some(Upload::Refused(UploadRefusal::Failed(step))) => (
                InstallRefusal::Failed,
                Some(step),
                LeaseRefusal::EnvironmentInstallFailed,
            ),
            None => (
                InstallRefusal::Unanswered,
                None,
                LeaseRefusal::EnvironmentUnreachable,
            ),
        };
        refused(audit, name, reason, seen);
        Err(refusal)
    }
}

impl HostInstaller {
    /// What the host's probe says; `None` when it said nothing this reads.
    async fn probe(&self, host: &SshDestination) -> Option<Probe> {
        let name = host.as_str();
        let probed = tokio::time::timeout(
            self.timings.probe,
            self.shell.run(host, probe_command(self.protocol), None),
        )
        .await;
        match probed {
            Ok(Ok(output)) => {
                let probe = Probe::parse(&output);
                if probe.is_none() {
                    tracing::warn!(host = name, "the host's probe said something else");
                }
                probe
            }
            Ok(Err(error)) => {
                tracing::warn!(host = name, %error, "the host could not be probed");
                None
            }
            Err(_) => {
                tracing::warn!(host = name, "the host's probe did not answer in time");
                None
            }
        }
    }
}

/// Record an install refused; a record that fails is logged, as the refusal
/// stands either way.
fn refused(audit: &dyn EnvironmentAudit, host: &str, reason: InstallRefusal, seen: Option<String>) {
    tracing::warn!(
        host,
        ?reason,
        ?seen,
        "this build was not installed on the host"
    );
    let event = EnvironmentEvent::InstallRefused {
        host: host.into(),
        reason,
        seen,
    };
    if let Err(error) = audit.record(&event) {
        tracing::error!(%error, "an install refused could not be recorded");
    }
}
