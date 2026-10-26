//! The Cloudflare provider: `cloudflared`, pinned and checked, run as a
//! helper service whose public address is read from its own metrics
//! (`docs/design/auth/tunnel.md`, "Running cloudflared"). This file is the
//! only one that knows Cloudflare exists.
//!
//! ```text
//! prepare ──▶ <tunnel>/cloudflared-<version>/cloudflared, digest checked
//!             (downloaded from the pinned URL, size and digest checked, when missing)
//! launch  ──▶ quick: cloudflared tunnel --url http://<ingress>
//!             named: cloudflared tunnel run --token-file <tunnel>/token (written by Settings, `files`)
//!             both:  --no-autoupdate --loglevel warn --metrics 127.0.0.1:<port>, HOME=<tunnel>, nothing else inherited
//! key     ──▶ program path (so its version), mode, ingress, hostname, token digest
//! resume  ──▶ a taken-back cloudflared is asked on the metrics port its launch named
//! ready   ──▶ GET /ready is 200, and for quick GET /quicktunnel names its hostname
//! ```
//!
//! Arrows are steps in order. The token is passed in a file only the gateway
//! can read, never on the command line where other users see it; this file
//! only points at it.
use super::files::TunnelFiles;
use crate::agent_install::application::{ArchiveSource, StagedArchive};
use crate::services::application::{Launch, PortFuture, Service, ServiceFailure};
use crate::tunnel::application::CloudflareTunnel;
use nessa_protocol::tunnel::TunnelAddress;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek},
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The pinned release, compiled in: a gateway cannot be pointed at another.
const PINS: &str = include_str!("../../../data/tunnel-releases.json");
/// How long a started `cloudflared` has to connect and name its address.
const READY_WITHIN: Duration = Duration::from_secs(60);
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

/// `cloudflared` as a helper service reporting the tunnel's address.
pub(crate) struct CloudflareService {
    /// `<namespace>/tunnel`, private to the gateway.
    files: TunnelFiles,
    directory: PathBuf,
    program: Program,
    archives: Arc<dyn ArchiveSource>,
    tunnel: CloudflareTunnel,
    ingress: SocketAddr,
    /// The metrics port the current launch was given.
    metrics: Mutex<Option<u16>>,
}

impl CloudflareService {
    pub(crate) fn new(
        files: TunnelFiles,
        program: Program,
        archives: Arc<dyn ArchiveSource>,
        tunnel: CloudflareTunnel,
        ingress: SocketAddr,
    ) -> Self {
        Self {
            directory: files.directory().to_owned(),
            files,
            program,
            archives,
            tunnel,
            ingress,
            metrics: Mutex::new(None),
        }
    }

    fn program_path(&self) -> Result<PathBuf, ServiceFailure> {
        match &self.program {
            Program::Pinned(build) => Ok(installed(&self.directory, build)),
            Program::Given(path) => Ok(path.clone()),
            Program::Unsupported => Err(ServiceFailure::Unavailable("unsupported")),
        }
    }
}

impl Service<TunnelAddress> for CloudflareService {
    fn name(&self) -> &'static str {
        "cloudflared"
    }

    fn ready_within(&self) -> Duration {
        READY_WITHIN
    }

    fn prepare(&self) -> PortFuture<'_, Result<(), ServiceFailure>> {
        Box::pin(async move {
            private_directory(&self.directory)
                .map_err(|_| ServiceFailure::Unavailable("storage"))?;
            match &self.program {
                Program::Unsupported => Err(ServiceFailure::Unavailable("unsupported")),
                Program::Given(path) => path
                    .is_file()
                    .then_some(())
                    .ok_or(ServiceFailure::Unavailable("unsupported")),
                Program::Pinned(build) => {
                    let directory = self.directory.clone();
                    let build = build.clone();
                    let archives = self.archives.clone();
                    tokio::task::spawn_blocking(move || install(&directory, &build, &*archives))
                        .await
                        .map_err(|_| ServiceFailure::Unavailable("download"))?
                        .map(drop)
                }
            }
        })
    }

    fn key(&self) -> Result<String, ServiceFailure> {
        let program = self.program_path()?;
        let tunnel = match &self.tunnel {
            CloudflareTunnel::Quick => "quick".to_owned(),
            CloudflareTunnel::Named { address, token } => {
                format!("named {} token-sha256 {}", address.as_str(), token.digest())
            }
        };
        Ok(format!(
            "cloudflared {} ingress {} {tunnel}",
            program.display(),
            self.ingress
        ))
    }

    fn resume(&self, launch: &Launch) {
        let port = launch
            .args
            .iter()
            .skip_while(|arg| *arg != "--metrics")
            .nth(1)
            .and_then(|metrics| metrics.to_str()?.parse::<SocketAddr>().ok())
            .filter(|metrics| metrics.ip().is_loopback())
            .map(|metrics| metrics.port());
        *self.metrics.lock().unwrap_or_else(PoisonError::into_inner) = port;
    }

    fn launch(&self) -> Result<Launch, ServiceFailure> {
        let program = self.program_path()?;
        let port = free_loopback_port().map_err(|_| ServiceFailure::Start)?;
        *self.metrics.lock().unwrap_or_else(PoisonError::into_inner) = Some(port);
        let mut args: Vec<OsString> = vec![
            "tunnel".into(),
            "--no-autoupdate".into(),
            // What it logs goes to a file that lives as long as it does.
            "--loglevel".into(),
            "warn".into(),
            "--metrics".into(),
            format!("127.0.0.1:{port}").into(),
        ];
        match &self.tunnel {
            CloudflareTunnel::Quick => {
                args.push("--url".into());
                args.push(format!("http://{}", self.ingress).into());
            }
            CloudflareTunnel::Named { .. } => {
                args.push("run".into());
                args.push("--token-file".into());
                args.push(self.files.token_path().into());
            }
        }
        Ok(Launch {
            program,
            args,
            // HOME is the tunnel directory, so a person's own
            // `~/.cloudflared` setup is never read: a config file there
            // would turn a quick tunnel off.
            environment: vec![("HOME".into(), self.directory.clone().into())],
            directory: self.directory.clone(),
        })
    }

    fn ready(&self) -> PortFuture<'_, Option<TunnelAddress>> {
        let port = *self.metrics.lock().unwrap_or_else(PoisonError::into_inner);
        Box::pin(async move {
            let port = port?;
            let (status, _) = probe(port, "/ready").await.ok()?;
            if status != 200 {
                return None;
            }
            match &self.tunnel {
                CloudflareTunnel::Named { address, .. } => Some(address.clone()),
                CloudflareTunnel::Quick => {
                    let (status, body) = probe(port, "/quicktunnel").await.ok()?;
                    (status == 200).then(|| quick_address(&body)).flatten()
                }
            }
        })
    }
}

/// The address a quick tunnel's `/quicktunnel` answer names.
fn quick_address(body: &[u8]) -> Option<TunnelAddress> {
    #[derive(Deserialize)]
    struct Quick {
        hostname: String,
    }
    let start = body.iter().position(|byte| *byte == b'{')?;
    let end = body.iter().rposition(|byte| *byte == b'}')?;
    let quick: Quick = serde_json::from_slice(body.get(start..=end)?).ok()?;
    if quick.hostname.is_empty() {
        return None;
    }
    // cloudflared names a bare hostname, served over HTTPS. An answer that
    // names a whole origin is taken as written, which is how the stress
    // check's stand-in names its loopback edge; the address type still
    // refuses plain HTTP to anything but loopback.
    let origin = if quick.hostname.contains("://") {
        quick.hostname
    } else {
        format!("https://{}", quick.hostname)
    };
    TunnelAddress::try_from(origin).ok()
}

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
) -> Result<PathBuf, ServiceFailure> {
    let program = installed(directory, build);
    let storage = |_| ServiceFailure::Unavailable("storage");
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
            ServiceFailure::Unavailable("download")
        })
        .and_then(|()| {
            let file = staged.file_mut();
            file.rewind().map_err(storage)?;
            if !digest_matches(file, build.archive_bytes, &build.archive_digest).map_err(storage)? {
                return Err(ServiceFailure::Unavailable("unverified"));
            }
            file.rewind().map_err(storage)?;
            unpack(file, build, release)
        });
    let _ = fs::remove_file(&download);
    result.map(|()| program)
}

/// Unpack the program from the checked download into place, checked again.
fn unpack(file: &mut File, build: &PinnedBuild, release: &Path) -> Result<(), ServiceFailure> {
    let storage = |_| ServiceFailure::Unavailable("storage");
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
                .map_err(|_| ServiceFailure::Unavailable("unverified"))?
            {
                let entry = entry.map_err(|_| ServiceFailure::Unavailable("unverified"))?;
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
                return Err(ServiceFailure::Unavailable("unverified"));
            }
        }
    }
    out.sync_all().map_err(storage)?;
    drop(out);
    if !matches(&partial, build.program_bytes, &build.program_digest).map_err(storage)? {
        let _ = fs::remove_file(&partial);
        return Err(ServiceFailure::Unavailable("unverified"));
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
