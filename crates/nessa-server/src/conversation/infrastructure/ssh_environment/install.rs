//! First use of a host (issue #703): when a host's stream ends with nothing
//! said on it, the environment asks whether this build is installed there,
//! and if it is not, puts it there before connecting again.
//!
//! ```text
//! ensure(host, lease)            every record names the host and the lease
//!   ──▶ probe ──▶ present ──▶ Present: nothing written (the host's own trouble)
//!             ──▶ no answer ──▶ refused environment_unreachable
//!             ──▶ absent <platform> ──▶ this build runs there? ──▶ its own executable
//!                    no ──▶ a release build it pins runs there? ──▶ that build
//!                    no ──▶ InstallRefused{platform} ──▶ environment_platform_unsupported
//!   ──▶ that build opened, and its SHA-256 the one expected?
//!          no ──▶ InstallRefused{source | release} ──▶ environment_install_failed
//!   ──▶ InstallStarted recorded (or nothing is sent)
//!   ──▶ upload ──▶ installed ──▶ Installed recorded ──▶ Installed
//!                                 (unrecorded ──▶ environment_install_failed)
//!              ──▶ refused fingerprint | unrunnable | digest tool | failed
//!                     ──▶ InstallRefused ──▶ environment_install_failed
//!              ──▶ refused version ──▶ InstallRefused ──▶ environment_version_mismatch
//!              ──▶ no answer ──▶ probe again: present ──▶ InstallFound ──▶ Installed
//!                                           (unrecorded ──▶ environment_install_failed)
//!                                           otherwise ──▶ InstallUnsettled (its script
//!                                                         may yet place the copy)
//!                                                         ──▶ environment_unreachable
//! ```
//!
//! Arrows are steps, in order. What is installed is this gateway's own
//! executable where it runs: the same system and processor (and on Linux
//! the GNU C library, which it links). On a host of another platform it is
//! the release build of these same sources for that platform, when this
//! build is a release that pins one (#734, `release.rs`); a development
//! build pins none, so such a host is refused. The serve command and the
//! probe look for each of those copies in turn. The host's verdicts — the
//! digest, then the protocol the copy itself states — are taken there
//! before the copy is put where it is found (`env_serve::install`), and the
//! hello after it is checked as any other.
//!
//! An install whose outcome cannot be recorded is not used, as a connection
//! that cannot be recorded is not: the lease is refused, and any install
//! record that fails, a refusal's included, answers
//! `environment_install_failed`. The copy stays on
//! the host, verified, so the next lease is served by it and recorded
//! `Connected`; the audit keeps the `InstallStarted` with no outcome.
use super::audit::{EnvironmentAudit, EnvironmentEvent, InstallRefusal};
use super::release::{pins, HttpsReleases, ReleaseBuild, ReleaseDownload};
use crate::env::{HOST_BUILDS, HOST_BUILDS_FROM, LEASE_PROTOCOL};
use crate::env_serve::install::{
    probe_command, serve_command, upload_command, Platform, Probe, Upload, UploadRefusal,
};
use nessa_sdk::domain::agent_execution::leases::{LeaseRefusal, SshDestination};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    future::Future,
    io::{self, Seek, SeekFrom},
    path::{Path, PathBuf},
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

/// This process's own executable, opened when the gateway starts: what an
/// update or removal later puts at its path is never what is measured or
/// sent. On Linux it is `/proc/self/exe`, the running file itself; elsewhere
/// the file `current_exe` names at start, which this handle keeps even once
/// the path names another.
pub(crate) struct OwnExecutable {
    held: Option<File>,
}

impl OwnExecutable {
    /// Open it now.
    pub(crate) fn opened_now() -> Self {
        let path = if cfg!(target_os = "linux") {
            Ok(Path::new("/proc/self/exe").to_path_buf())
        } else {
            std::env::current_exe()
        };
        match path {
            Ok(path) => Self::opened_at(&path),
            Err(error) => Self::held(Err(error)),
        }
    }

    /// The build at `path`, opened now.
    pub(crate) fn opened_at(path: &Path) -> Self {
        Self::held(File::open(path))
    }

    fn held(opened: io::Result<File>) -> Self {
        if let Err(error) = &opened {
            tracing::error!(%error, "this build's executable could not be opened; no host gets it");
        }
        Self { held: opened.ok() }
    }
}

impl BuildSource for OwnExecutable {
    fn open(&self) -> io::Result<Build> {
        let held = self
            .held
            .as_ref()
            .ok_or_else(|| io::Error::other("this build's executable was not opened at start"))?;
        // Measured by position, so the shared offset is left for the send;
        // the installer sends one copy at a time.
        let digest = sha256_of(held)?;
        let mut file = held.try_clone()?;
        file.seek(SeekFrom::Start(0))?;
        Ok(Build { file, digest })
    }
}

/// The SHA-256 of what `file` reads from its start, read by position so
/// its offset is left where it is.
///
/// # Errors
/// It could not be read.
pub(crate) fn sha256_of(file: &File) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    let mut at = 0;
    loop {
        let read = std::os::unix::fs::FileExt::read_at(file, &mut buffer, at)?;
        if read == 0 {
            return Ok(format!("{:x}", hasher.finalize()));
        }
        hasher.update(&buffer[..read]);
        at += read as u64;
    }
}

/// A build of these sources for another platform, which this one pins
/// (#734): sent to a host this build does not run on.
pub(crate) struct ReleaseCopy {
    /// What it runs on.
    pub(crate) platform: Platform,
    /// Its SHA-256, as pinned: where it is kept on a host, and what the
    /// bytes opened must measure.
    pub(crate) digest: String,
    /// Where its bytes come from.
    pub(crate) build: Arc<dyn BuildSource>,
}

impl ReleaseCopy {
    /// The release builds this build pins, downloaded by `download` and
    /// kept in `cache`: none unless it was compiled as a release, and none
    /// (said in the log) if what it was compiled with does not read.
    pub(crate) fn pinned(
        spec: Option<&str>,
        from: Option<&str>,
        download: Arc<dyn ReleaseDownload>,
        cache: &Path,
    ) -> Vec<Self> {
        let (Some(spec), Some(from)) = (spec, from) else {
            return Vec::new();
        };
        match pins(spec) {
            Ok(pins) => pins
                .into_iter()
                .map(|pin| {
                    let build = ReleaseBuild::new(pin, from, download.clone(), cache.into());
                    Self {
                        platform: build.pin().platform.clone(),
                        digest: build.pin().digest.clone(),
                        build: Arc::new(build),
                    }
                })
                .collect(),
            Err(error) => {
                tracing::error!(%error, "this build's release pins do not read; it installs only itself");
                Vec::new()
            }
        }
    }
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
    /// The release builds for other platforms, in the order a host is
    /// searched for them after this build's own.
    releases: Vec<ReleaseCopy>,
    protocol: &'static str,
    timings: InstallTimings,
    /// This build's SHA-256, taken once: where its copy is kept on a host.
    digest: tokio::sync::OnceCell<String>,
    /// Held from opening this build to the end of its upload, so one copy is
    /// sent at a time: the copies opened share where the executable is read.
    sending: tokio::sync::Mutex<()>,
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
            releases: Vec::new(),
            protocol,
            timings,
            digest: tokio::sync::OnceCell::new(),
            sending: tokio::sync::Mutex::new(()),
        }
    }

    /// With `releases` for hosts this build does not run on.
    pub(crate) fn with_releases(mut self, releases: Vec<ReleaseCopy>) -> Self {
        self.releases = releases;
        self
    }

    /// This build, its platform and protocol, and the release builds it
    /// pins, kept in `cache` once downloaded; sent by `shell`.
    pub(crate) fn this_build(shell: Arc<dyn RemoteShell>, cache: PathBuf) -> Self {
        Self::new(
            shell,
            Arc::new(OwnExecutable::opened_now()),
            Platform::this_build(),
            LEASE_PROTOCOL,
            InstallTimings::default(),
        )
        .with_releases(ReleaseCopy::pinned(
            HOST_BUILDS,
            HOST_BUILDS_FROM,
            Arc::new(HttpsReleases::default()),
            &cache,
        ))
    }

    /// This build's SHA-256, taken from its executable the first time it is
    /// asked for.
    ///
    /// # Errors
    /// `environment_install_failed`: the executable could not be read, so
    /// which copy on a host is this build cannot be known.
    async fn digest(&self) -> Result<&str, LeaseRefusal> {
        let digest = self
            .digest
            .get_or_try_init(|| async {
                let build = self.build.clone();
                tokio::task::spawn_blocking(move || build.open())
                    .await
                    .map_err(io::Error::other)
                    .and_then(|opened| opened)
                    .map(|build| build.digest)
            })
            .await;
        digest.map(String::as_str).map_err(|error| {
            tracing::error!(%error, "this build's executable could not be read");
            LeaseRefusal::EnvironmentInstallFailed
        })
    }

    /// Every copy of this build a host may keep, by SHA-256: its own, then
    /// each release build's.
    ///
    /// # Errors
    /// As [`Self::digest`].
    async fn builds(&self) -> Result<Vec<&str>, LeaseRefusal> {
        let own = self.digest().await?;
        let mut builds = vec![own];
        builds.extend(
            self.releases
                .iter()
                .map(|release| release.digest.as_str())
                .filter(|digest| *digest != own),
        );
        Ok(builds)
    }

    /// The command that serves this build on a host: the first of its
    /// copies kept there, each by its SHA-256, so only these exact bytes are
    /// run there.
    ///
    /// # Errors
    /// As [`Self::digest`].
    pub(crate) async fn serve_command(&self) -> Result<String, LeaseRefusal> {
        Ok(serve_command(&self.builds().await?))
    }

    /// Make sure this build is installed on `host`, for `lease`.
    ///
    /// # Errors
    /// The typed refusal; each install refused is recorded in `audit`.
    pub(crate) async fn ensure(
        &self,
        host: &SshDestination,
        lease: &str,
        audit: &dyn EnvironmentAudit,
    ) -> Result<Installation, LeaseRefusal> {
        let name = host.as_str();
        let platform = match self.probe(host).await {
            Some(Probe::Present) => return Ok(Installation::Present),
            Some(Probe::Absent(platform)) => platform,
            None => return Err(LeaseRefusal::EnvironmentUnreachable),
        };
        // This build where it runs, else the release build for the host's
        // platform. Served by its digest, so it is known by now; the copy
        // sent must be those bytes, or it would be kept where another build
        // is looked for (on macOS the path can name a newer executable than
        // the one running; a download can be anything).
        let (source, expected, reason) = if self.platform.runs_on(&platform) {
            (
                self.build.clone(),
                self.digest().await?,
                InstallRefusal::Source,
            )
        } else if let Some(release) = self
            .releases
            .iter()
            .find(|release| release.platform.runs_on(&platform))
        {
            (
                release.build.clone(),
                release.digest.as_str(),
                InstallRefusal::Release,
            )
        } else {
            return Err(refused(
                audit,
                name,
                lease,
                InstallRefusal::Platform,
                Some(platform.to_string()),
                LeaseRefusal::EnvironmentPlatformUnsupported,
            ));
        };
        let sending = self.sending.lock().await;
        let opened = tokio::task::spawn_blocking(move || source.open())
            .await
            .map_err(io::Error::other)
            .and_then(|opened| opened);
        let Build { file, digest } = match opened {
            Ok(build) if build.digest == expected => build,
            Ok(build) => {
                tracing::error!(
                    measured = build.digest,
                    expected,
                    ?reason,
                    "the build to install is not the one expected; nothing is sent"
                );
                // A download's digest is what it was; this build's own
                // executable changing under it says nothing of the host.
                let seen = (reason == InstallRefusal::Release).then_some(build.digest);
                return Err(refused(
                    audit,
                    name,
                    lease,
                    reason,
                    seen,
                    LeaseRefusal::EnvironmentInstallFailed,
                ));
            }
            Err(error) => {
                tracing::error!(%error, ?reason, "the build to install could not be had");
                return Err(refused(
                    audit,
                    name,
                    lease,
                    reason,
                    None,
                    LeaseRefusal::EnvironmentInstallFailed,
                ));
            }
        };
        let started = EnvironmentEvent::InstallStarted {
            host: name.into(),
            lease: lease.into(),
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
        drop(sending);
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
                lease: lease.into(),
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
                    lease: lease.into(),
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
            // Not refused: the host may still be running the upload and
            // place the copy after this, so the record says it is unsettled.
            None => {
                let unsettled = EnvironmentEvent::InstallUnsettled {
                    host: name.into(),
                    lease: lease.into(),
                    protocol: self.protocol.into(),
                    digest,
                };
                tracing::warn!(host = name, lease, "the upload's outcome is not known");
                return Err(match audit.record(&unsettled) {
                    Ok(()) => LeaseRefusal::EnvironmentUnreachable,
                    Err(error) => {
                        tracing::error!(%error, "an unsettled install could not be recorded");
                        LeaseRefusal::EnvironmentInstallFailed
                    }
                });
            }
        };
        Err(refused(audit, name, lease, reason, seen, refusal))
    }
}

impl HostInstaller {
    /// What the host's probe says; `None` when it said nothing this reads.
    async fn probe(&self, host: &SshDestination) -> Option<Probe> {
        let name = host.as_str();
        let probed = tokio::time::timeout(
            self.timings.probe,
            self.shell.run(
                host,
                probe_command(self.protocol, &self.builds().await.ok()?),
                None,
            ),
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

/// Record an install refused, and answer `refusal`; a refusal that cannot
/// be recorded is `environment_install_failed`, as an install whose start
/// or outcome cannot be recorded is, so a failed audit is never answered
/// as the host's own reason.
fn refused(
    audit: &dyn EnvironmentAudit,
    host: &str,
    lease: &str,
    reason: InstallRefusal,
    seen: Option<String>,
    refusal: LeaseRefusal,
) -> LeaseRefusal {
    tracing::warn!(
        host,
        lease,
        ?reason,
        ?seen,
        "this build was not installed on the host"
    );
    let event = EnvironmentEvent::InstallRefused {
        host: host.into(),
        lease: lease.into(),
        reason,
        seen,
    };
    match audit.record(&event) {
        Ok(()) => refusal,
        Err(error) => {
            tracing::error!(%error, "an install refused could not be recorded");
            LeaseRefusal::EnvironmentInstallFailed
        }
    }
}
