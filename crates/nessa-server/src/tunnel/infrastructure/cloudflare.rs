//! The Cloudflare provider: `cloudflared`, pinned and checked, run as a
//! shepherd service whose public address is read from its own metrics
//! (`docs/design/auth/tunnel.md`, "Running cloudflared"). This file is the
//! only one that knows Cloudflare exists.
//!
//! ```text
//! launch ──▶ a refusal's reason recorded in LaunchRefusal (emptied when it goes ahead)
//!        ──▶ <tunnel>/cloudflared-<version>/cloudflared, digest checked
//!            (downloaded from the pinned URL, size and digest checked, when missing)
//!        ──▶ quick: cloudflared tunnel --url http://<ingress>
//!            named: cloudflared tunnel run --token-file <tunnel>/token (written by Settings, `files`),
//!                   revision "token <fingerprint>"
//!            both:  --no-autoupdate --loglevel warn --metrics 127.0.0.1:<port>, HOME=<tunnel>,
//!                   nothing else inherited, output to <tunnel>/cloudflared.log
//!            (<port>: the kept program's, read through KeptTunnel, when the launch on it is exactly
//!             the one recorded and no launch of this run has run yet, so shepherd takes it back;
//!             else one free now)
//! ready  ──▶ asked on the metrics port the running program's own record names:
//!            GET /ready is 200, and for quick GET /quicktunnel names its hostname
//! ```
//!
//! Arrows are steps in order. The token is passed in a file only the gateway
//! can read, never on the command line where other users see it; this file
//! only points at it.
use super::files::TunnelFiles;
use crate::agent_install::application::{ArchiveSource, StagedArchive};
use crate::tunnel::application::{CloudflareTunnel, LaunchRefusal, TunnelFailure};
use async_trait::async_trait;
use nessa_protocol::tunnel::TunnelAddress;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use shepherd::{
    EnvPolicy, KeptName, KeptProcesses, ProcessSpec, Service, ServiceFailure, ServiceInstance,
    ServiceLaunch, SpecFingerprint,
};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek},
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, PoisonError,
    },
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The pinned release, compiled in: a gateway cannot be pointed at another.
const PINS: &str = include_str!("../../../data/tunnel-releases.json");
/// How long a started `cloudflared` has to connect and name its address.
pub(crate) const READY_WITHIN: Duration = Duration::from_secs(60);
/// Its log, beside it: what it writes goes here, for as long as it runs.
const LOG: &str = "cloudflared.log";
/// How long one metrics request may take.
const PROBE_DEADLINE: Duration = Duration::from_secs(2);
/// The most of a metrics answer that is read.
const MAX_PROBE_BYTES: usize = 8 * 1024;
/// The program's name inside its archive and in its directory.
const PROGRAM: &str = "cloudflared";

/// The pin file.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Pins {
    #[allow(dead_code)]
    comment: String,
    version: String,
    builds: Vec<PinnedBuild>,
}

/// One platform's pinned build.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PinnedBuild {
    operating_system: String,
    architecture: String,
    url: String,
    packaging: Packaging,
    archive_bytes: u64,
    archive_digest: String,
    program_bytes: u64,
    program_digest: String,
    #[serde(skip)]
    version: String,
}

/// What the pinned URL serves: the program itself, or a gzipped tar
/// holding it.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum Packaging {
    Program,
    Tgz,
}

impl PinnedBuild {
    /// This machine's pinned build, or `None` when there is none.
    pub(crate) fn for_this_machine() -> Option<Self> {
        let operating_system = match std::env::consts::OS {
            "macos" => "macos",
            "linux" => "linux",
            _ => return None,
        };
        Self::for_platform(operating_system, std::env::consts::ARCH)
    }

    fn for_platform(operating_system: &str, architecture: &str) -> Option<Self> {
        let pins: Pins = serde_json::from_str(PINS).expect("the pinned tunnel release parses");
        pins.builds
            .into_iter()
            .find(|build| {
                build.operating_system == operating_system && build.architecture == architecture
            })
            .map(|build| PinnedBuild {
                version: pins.version.clone(),
                ..build
            })
    }
}

/// Which program runs.
pub(crate) enum Program {
    /// The pinned build, downloaded and checked into the tunnel directory.
    Pinned(PinnedBuild),
    /// A program named by `NESSA_TUNNEL_PROGRAM` in a debug build: how the
    /// stress check stands in for Cloudflare. A release build never reads it.
    Given(PathBuf),
    /// No pinned build for this machine.
    Unsupported,
}

impl Program {
    /// The pinned build, or in a debug build the stand-in named in the
    /// environment.
    pub(crate) fn chosen() -> Self {
        if cfg!(debug_assertions) {
            if let Some(given) = std::env::var_os("NESSA_TUNNEL_PROGRAM") {
                return Self::Given(given.into());
            }
        }
        PinnedBuild::for_this_machine().map_or(Self::Unsupported, Self::Pinned)
    }
}

/// Makes the source a pinned build is downloaded from. Called, and its
/// source dropped, only on a blocking thread: the HTTPS source holds a
/// blocking client, whose own runtime must never be dropped inside the
/// gateway's.
pub(crate) type MakeArchives = Arc<dyn Fn() -> Option<Box<dyn ArchiveSource>> + Send + Sync>;

/// `cloudflared` as a shepherd service reporting the tunnel's address.
pub(crate) struct CloudflareService {
    /// `<namespace>/tunnel`, private to the gateway.
    files: TunnelFiles,
    directory: PathBuf,
    program: Program,
    archives: MakeArchives,
    tunnel: CloudflareTunnel,
    ingress: SocketAddr,
    /// The program a last run left running: a run's first launch asks it.
    kept: Arc<dyn KeptTunnel>,
    /// Whether a launch of this run has started or been taken back: until
    /// one has, a launch reuses the kept program's metrics port.
    ran: AtomicBool,
    /// Why the last launch was refused, which the status is read from.
    refused: LaunchRefusal,
}

impl CloudflareService {
    pub(crate) fn new(
        files: TunnelFiles,
        program: Program,
        archives: MakeArchives,
        tunnel: CloudflareTunnel,
        ingress: SocketAddr,
        kept: Arc<dyn KeptTunnel>,
        refused: LaunchRefusal,
    ) -> Self {
        Self {
            directory: files.directory().to_owned(),
            files,
            program,
            archives,
            tunnel,
            ingress,
            kept,
            ran: AtomicBool::new(false),
            refused,
        }
    }

    fn program_path(&self) -> Result<PathBuf, TunnelFailure> {
        match &self.program {
            Program::Pinned(build) => Ok(installed(&self.directory, build)),
            Program::Given(path) => Ok(path.clone()),
            Program::Unsupported => Err(TunnelFailure::Unavailable("unsupported")),
        }
    }

    /// Make its program ready to start: downloaded and checked, once.
    async fn prepare(&self) -> Result<(), TunnelFailure> {
        private_directory(&self.directory).map_err(|_| TunnelFailure::Unavailable("storage"))?;
        match &self.program {
            Program::Unsupported => Err(TunnelFailure::Unavailable("unsupported")),
            Program::Given(path) => path
                .is_file()
                .then_some(())
                .ok_or(TunnelFailure::Unavailable("unsupported")),
            Program::Pinned(build) => {
                let directory = self.directory.clone();
                let build = build.clone();
                let archives = self.archives.clone();
                tokio::task::spawn_blocking(move || {
                    let archives = archives().ok_or(TunnelFailure::Unavailable("download"))?;
                    install(&directory, &build, &*archives)
                })
                .await
                .map_err(|_| TunnelFailure::Unavailable("download"))?
                .map(drop)
            }
        }
    }

    /// The launch on the port the program a last run left running was
    /// started with, as its kept record says, when that launch is exactly
    /// the one recorded: shepherd then takes the program back. Asked only
    /// until a launch of this run has started or been taken back. A record
    /// that cannot be read takes back nothing.
    async fn kept_launch(&self) -> Result<Option<ServiceLaunch>, TunnelFailure> {
        if self.ran.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let left = match self.kept.left_running().await {
            Ok(Some(left)) => left,
            Ok(None) => return Ok(None),
            // The lookup logged why; the program is started anew.
            Err(KeptLookupFailed) => return Ok(None),
        };
        let Some(port) = metrics_port_in(&left.args) else {
            return Ok(None);
        };
        let launch = self.launch_on(port)?;
        Ok((StartedAs::of(&launch) == left.started_as).then_some(launch))
    }

    /// What to run this time: the kept program's own launch when it is
    /// still this one, else one reporting on a port free now. A port is
    /// never reused for a different launch, which would start it on a port
    /// another program could take while the old one is ended.
    async fn launch_spec(&self) -> Result<ServiceLaunch, TunnelFailure> {
        if let Some(launch) = self.kept_launch().await? {
            return Ok(launch);
        }
        let port = free_loopback_port().map_err(|_| TunnelFailure::Start)?;
        self.launch_on(port)
    }

    /// The launch reporting on `port`. Everything that makes it a different
    /// program is in the spec, and the token, which is only named there by
    /// its file, is in the revision, so shepherd takes back a program a last
    /// run left running only when it is still this one.
    fn launch_on(&self, port: u16) -> Result<ServiceLaunch, TunnelFailure> {
        let program = self.program_path()?;
        let mut args: Vec<OsString> = vec![
            "tunnel".into(),
            "--no-autoupdate".into(),
            // What it logs goes to a file that lives as long as it does.
            "--loglevel".into(),
            "warn".into(),
            "--metrics".into(),
            format!("127.0.0.1:{port}").into(),
        ];
        let mut revision = None;
        match &self.tunnel {
            CloudflareTunnel::Quick => {
                args.push("--url".into());
                args.push(format!("http://{}", self.ingress).into());
            }
            CloudflareTunnel::Named { token, .. } => {
                args.push("run".into());
                args.push("--token-file".into());
                args.push(self.files.token_path().into());
                revision = Some(format!("token {}", token.fingerprint()));
            }
        }
        let spec = ProcessSpec::new(program)
            .args(args)
            // HOME is the tunnel directory, so a person's own
            // `~/.cloudflared` setup is never read: a config file there
            // would turn a quick tunnel off.
            .env(EnvPolicy::Clear(vec![(
                "HOME".into(),
                self.directory.clone().into(),
            )]))
            .cwd(self.directory.clone());
        let launch = ServiceLaunch::new(spec).log(self.directory.join(LOG));
        Ok(match revision {
            Some(revision) => launch.revision(revision),
            None => launch,
        })
    }

    /// The address a running program reports once it is ready, asked on
    /// the metrics port its own launch named.
    async fn address(&self, args: &[String]) -> Option<TunnelAddress> {
        let port = metrics_port_in(args)?;
        let (status, _) = probe(port, "/ready").await.ok()?;
        if status != 200 {
            return None;
        }
        match &self.tunnel {
            CloudflareTunnel::Named { address, .. } => Some(address.clone()),
            CloudflareTunnel::Quick => {
                let (status, body) = probe(port, "/quicktunnel").await.ok()?;
                let stand_in = matches!(self.program, Program::Given(_));
                (status == 200)
                    .then(|| quick_address(&body, stand_in))
                    .flatten()
            }
        }
    }
}

#[async_trait]
impl Service for CloudflareService {
    type Ready = TunnelAddress;

    /// A refusal's reason is recorded for the status; its words are only
    /// logged.
    async fn launch(&self) -> Result<ServiceLaunch, ServiceFailure> {
        let launched = match self.prepare().await {
            Ok(()) => self.launch_spec().await,
            Err(failure) => Err(failure),
        };
        *self.refused.lock().unwrap_or_else(PoisonError::into_inner) =
            launched.as_ref().err().copied();
        launched.map_err(|failure| ServiceFailure::new(failure.as_str()))
    }

    async fn ready(
        &self,
        instance: &ServiceInstance,
    ) -> Result<Option<TunnelAddress>, ServiceFailure> {
        // Only a started or taken-back run is asked whether it is ready.
        self.ran.store(true, Ordering::SeqCst);
        let args = instance
            .record
            .as_ref()
            .map(|record| record.args.as_slice())
            .unwrap_or_default();
        Ok(self.address(args).await)
    }
}

/// The program a last run left running for the tunnel, as its record says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LeftRunning {
    /// The arguments it was started with.
    pub(crate) args: Vec<String>,
    pub(crate) started_as: StartedAs,
}

/// What a kept program was started as: compared with a launch, never read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StartedAs(SpecFingerprint);

impl StartedAs {
    /// What `launch` starts a program as.
    pub(crate) fn of(launch: &ServiceLaunch) -> Self {
        Self(SpecFingerprint::of(
            &launch.spec,
            launch.revision.as_deref(),
        ))
    }
}

/// The program left running could not be looked up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct KeptLookupFailed;

/// Where the program a last run left running is recorded, outside this
/// process. Composition injects shepherd's records ([`ShepherdKept`]);
/// tests substitute their own.
#[async_trait]
pub(crate) trait KeptTunnel: Send + Sync {
    /// The live program kept for the tunnel that no other owner holds.
    async fn left_running(&self) -> Result<Option<LeftRunning>, KeptLookupFailed>;
}

/// The tunnel's program as shepherd keeps it, under one name.
pub(crate) struct ShepherdKept {
    kept: KeptProcesses,
    name: KeptName,
}

impl ShepherdKept {
    pub(crate) fn new(kept: KeptProcesses, name: KeptName) -> Self {
        Self { kept, name }
    }
}

#[async_trait]
impl KeptTunnel for ShepherdKept {
    async fn left_running(&self) -> Result<Option<LeftRunning>, KeptLookupFailed> {
        let live = self.kept.list().await.map_err(|error| {
            tracing::warn!(%error, "the kept tunnel program's record could not be read");
            KeptLookupFailed
        })?;
        Ok(live
            .into_iter()
            .find(|entry| entry.record.name == self.name && !entry.held)
            .map(|entry| LeftRunning {
                args: entry.record.args,
                started_as: StartedAs(entry.record.fingerprint),
            }))
    }
}

/// The loopback port a launch's `--metrics` names.
fn metrics_port_in(args: &[String]) -> Option<u16> {
    args.iter()
        .skip_while(|arg| *arg != "--metrics")
        .nth(1)?
        .parse::<SocketAddr>()
        .ok()
        .filter(|metrics| metrics.ip().is_loopback())
        .map(|metrics| metrics.port())
}

/// The address a quick tunnel's `/quicktunnel` answer names: a bare
/// `trycloudflare.com` hostname, served over HTTPS. Whatever answers on the
/// metrics port is believed only that far, since a local program could
/// answer there first. A `stand_in` (`NESSA_TUNNEL_PROGRAM`, debug builds
/// only) may name a whole origin, which is how the stress check's names its
/// loopback edge; the address type still refuses plain HTTP to anything but
/// loopback.
fn quick_address(body: &[u8], stand_in: bool) -> Option<TunnelAddress> {
    #[derive(Deserialize)]
    struct Quick {
        hostname: String,
    }
    let start = body.iter().position(|byte| *byte == b'{')?;
    let end = body.iter().rposition(|byte| *byte == b'}')?;
    let quick: Quick = serde_json::from_slice(body.get(start..=end)?).ok()?;
    let origin = if stand_in && quick.hostname.contains("://") {
        quick.hostname
    } else {
        let name = quick.hostname.strip_suffix(QUICK_DOMAIN)?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return None;
        }
        format!("https://{}", quick.hostname)
    };
    TunnelAddress::try_from(origin).ok()
}

/// Where every quick tunnel's hostname is.
const QUICK_DOMAIN: &str = ".trycloudflare.com";

/// One `GET` to the metrics server on loopback: its status and body.
async fn probe(port: u16, path: &str) -> io::Result<(u16, Vec<u8>)> {
    tokio::time::timeout(PROBE_DEADLINE, async {
        let mut stream =
            tokio::net::TcpStream::connect(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).await?;
        stream
            .write_all(
                format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await?;
        let mut answer = Vec::new();
        (&mut stream)
            .take(MAX_PROBE_BYTES as u64)
            .read_to_end(&mut answer)
            .await?;
        parse_answer(&answer).ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))
    })
    .await
    .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
}

/// An HTTP/1.1 answer's status and body.
fn parse_answer(answer: &[u8]) -> Option<(u16, Vec<u8>)> {
    let head_end = answer.windows(4).position(|window| window == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&answer[..head_end]).ok()?;
    let status = head
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some((status, answer[head_end + 4..].to_vec()))
}

/// A loopback port free now, for the metrics server.
fn free_loopback_port() -> io::Result<u16> {
    std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map(|listener| {
        listener
            .local_addr()
            .map(|address| address.port())
            .unwrap_or(0)
    })
}

/// Where the pinned build's program is kept.
fn installed(directory: &Path, build: &PinnedBuild) -> PathBuf {
    directory
        .join(format!("cloudflared-{}", build.version))
        .join(PROGRAM)
}

/// Make the pinned build's program ready: keep it when it is already there
/// and matches its digest, else download, check and unpack it. Nothing that
/// does not match its pinned size and digest is ever made runnable (row T12).
pub(crate) fn install(
    directory: &Path,
    build: &PinnedBuild,
    archives: &dyn ArchiveSource,
) -> Result<PathBuf, TunnelFailure> {
    let program = installed(directory, build);
    let storage = |_| TunnelFailure::Unavailable("storage");
    if program.is_file() {
        if matches(&program, build.program_bytes, &build.program_digest).map_err(storage)? {
            return Ok(program);
        }
        tracing::warn!("the kept tunnel program does not match its pin; downloading it again");
        fs::remove_file(&program).map_err(storage)?;
    }
    let release = program.parent().expect("the program has a directory");
    private_directory(release).map_err(storage)?;
    let download = release.join("download.part");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&download)
        .map_err(storage)?;
    let mut staged = StagedArchive::new(file, download.clone());
    let fetched = archives.download(&build.url, build.archive_bytes, &mut staged);
    let result = fetched
        .map_err(|error| {
            tracing::warn!(?error, "the tunnel program could not be downloaded");
            TunnelFailure::Unavailable("download")
        })
        .and_then(|()| {
            let file = staged.file_mut();
            file.rewind().map_err(storage)?;
            if !digest_matches(file, build.archive_bytes, &build.archive_digest).map_err(storage)? {
                return Err(TunnelFailure::Unavailable("unverified"));
            }
            file.rewind().map_err(storage)?;
            unpack(file, build, release)
        });
    let _ = fs::remove_file(&download);
    result.map(|()| program)
}

/// Unpack the program from the checked download into place, checked again.
fn unpack(file: &mut File, build: &PinnedBuild, release: &Path) -> Result<(), TunnelFailure> {
    let storage = |_| TunnelFailure::Unavailable("storage");
    let partial = release.join(format!("{PROGRAM}.part"));
    let mut out = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&partial)
        .map_err(storage)?;
    // At most one byte more than pinned is copied, so a longer program is
    // seen without writing all of it.
    let limit = build.program_bytes + 1;
    match build.packaging {
        Packaging::Program => {
            io::copy(&mut file.take(limit), &mut out).map_err(storage)?;
        }
        Packaging::Tgz => {
            let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
            let mut found = false;
            for entry in archive
                .entries()
                .map_err(|_| TunnelFailure::Unavailable("unverified"))?
            {
                let entry = entry.map_err(|_| TunnelFailure::Unavailable("unverified"))?;
                let named = entry
                    .path()
                    .map(|path| path.as_os_str() == PROGRAM)
                    .unwrap_or(false);
                if named && entry.header().entry_type().is_file() {
                    io::copy(&mut entry.take(limit), &mut out).map_err(storage)?;
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(TunnelFailure::Unavailable("unverified"));
            }
        }
    }
    out.sync_all().map_err(storage)?;
    drop(out);
    if !matches(&partial, build.program_bytes, &build.program_digest).map_err(storage)? {
        let _ = fs::remove_file(&partial);
        return Err(TunnelFailure::Unavailable("unverified"));
    }
    runnable(&partial).map_err(storage)?;
    fs::rename(
        &partial,
        installed(release.parent().unwrap_or(release), build),
    )
    .map_err(storage)
}

/// Whether the file at `path` is exactly `bytes` long with SHA-256 `digest`.
fn matches(path: &Path, bytes: u64, digest: &str) -> io::Result<bool> {
    digest_matches(&mut File::open(path)?, bytes, digest)
}

fn digest_matches(file: &mut File, bytes: u64, digest: &str) -> io::Result<bool> {
    let mut hasher = Sha256::new();
    let mut read = 0u64;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        read += count as u64;
        if read > bytes {
            return Ok(false);
        }
        hasher.update(&buffer[..count]);
    }
    let found: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(read == bytes && found == digest)
}

/// A directory only the gateway's user can enter.
fn private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn runnable(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/tunnel/cloudflare.rs"]
mod tests;
