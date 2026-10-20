//! The standalone `nessa` a release publishes for each platform (issue
//! #734), for a host whose platform is not this gateway's.
//!
//! ```text
//! release workflow: host-builds (one per target) ──▶ app builds, each
//!                   compiled with NESSA_HOST_BUILDS = target=sha256:bytes,…
//! gateway: pins(HOST_BUILDS) ──▶ ReleaseBuild per target
//!   open ──▶ kept in the cache under its pin, still those bytes? ──▶ it
//!        ──▶ otherwise download (at most its pinned length), measure
//!               the pin ──▶ kept under it ──▶ sent
//!               another ──▶ handed back as measured: the installer
//!                           refuses it (`release`), nothing is sent or kept
//! ```
//!
//! The pins are compiled in, so they hold only for the build that carries
//! them: the release workflow builds each target's `nessa` from the same
//! commit first, then compiles the app's own with their digests and
//! lengths. Every copy is built from these sources, so each speaks this
//! build's lease protocol, and the host still checks that on the copy
//! before placing it. A build compiled without them (any development build)
//! has none, and a host of another platform is refused as before.
use super::install::{sha256_of, Build, BuildSource};
use crate::agent_install::infrastructure::https_archives::install_tls_backend;
use crate::env_serve::install::Platform;
use reqwest::blocking::Client;
use reqwest::redirect::Policy;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, OnceLock,
    },
    time::Duration,
};

/// One platform's build of this release, as this build pins it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReleasePin {
    /// Its Rust target triple, which names it in the release.
    pub(crate) target: String,
    /// What it runs on.
    pub(crate) platform: Platform,
    /// Its SHA-256, lowercase hex.
    pub(crate) digest: String,
    /// Its length in bytes.
    pub(crate) bytes: u64,
}

/// The pins in `spec`, `<target>=<sha256>:<bytes>` comma-separated, as the
/// release workflow writes them. A target this build has no platform for
/// (a release that added one) is skipped, said in the log, so the others
/// still serve.
///
/// # Errors
/// What is wrong with it: an entry of another shape, a target named twice,
/// a digest that is not 64 lowercase hex digits, a length that is not a
/// positive number. A release build with any of these pins nothing.
pub(crate) fn pins(spec: &str) -> Result<Vec<ReleasePin>, String> {
    let mut pins: Vec<ReleasePin> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for entry in spec.split(',') {
        let (target, pin) = entry
            .split_once('=')
            .ok_or_else(|| format!("{entry:?} is not target=sha256:bytes"))?;
        let (digest, bytes) = pin
            .split_once(':')
            .ok_or_else(|| format!("{entry:?} is not target=sha256:bytes"))?;
        if !seen.insert(target) {
            return Err(format!("{target} is pinned twice"));
        }
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(format!("{target}'s digest {digest:?} is not a SHA-256"));
        }
        let bytes = bytes
            .parse::<u64>()
            .ok()
            .filter(|bytes| *bytes > 0)
            .ok_or_else(|| format!("{target}'s length {bytes:?} is not a number of bytes"))?;
        let Some(platform) = Platform::of_target(target) else {
            tracing::warn!(
                target,
                "a release build for a target this build does not know is not used"
            );
            continue;
        };
        pins.push(ReleasePin {
            target: target.into(),
            platform,
            digest: digest.into(),
            bytes,
        });
    }
    Ok(pins)
}

/// Fetches what a release publishes.
pub(crate) trait ReleaseDownload: Send + Sync {
    /// Write what `url` serves into `into`, from where it is, and fail on
    /// the read that would take it past `at_most` bytes.
    ///
    /// # Errors
    /// It could not be reached, refused, was longer than `at_most`, or
    /// could not be written.
    fn fetch(&self, url: &str, at_most: u64, into: &mut File) -> io::Result<()>;
}

/// A release build: downloaded the first time it is opened and kept in
/// `cache` under its pinned digest, so a gateway downloads each platform's
/// build once, however many hosts and restarts it serves.
pub(crate) struct ReleaseBuild {
    pin: ReleasePin,
    url: String,
    download: Arc<dyn ReleaseDownload>,
    cache: PathBuf,
    /// Held while it is opened, so hosts asking for it together wait for
    /// one download rather than each making their own.
    opening: std::sync::Mutex<()>,
}

/// How long a download's temporary file is kept before it is taken to be
/// one a stopped gateway left, in seconds.
const ABANDONED_DOWNLOAD: Duration = Duration::from_secs(60 * 60);

/// Remove from `cache` every build but those `keep` names (an earlier
/// release's), and the temporary files of downloads abandoned long ago.
pub(crate) fn prune(cache: &std::path::Path, keep: &[&str]) {
    let Ok(entries) = fs::read_dir(cache) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let stale = if name.starts_with(".download.") {
            entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age > ABANDONED_DOWNLOAD)
        } else {
            !keep.contains(&name.as_ref())
        };
        if stale {
            if let Err(error) = fs::remove_file(entry.path()) {
                tracing::warn!(%error, name = %name, "a stale release build could not be removed");
            }
        }
    }
}

impl ReleaseBuild {
    /// `pin`'s build, published under `from` as `nessa-<target>`.
    pub(crate) fn new(
        pin: ReleasePin,
        from: &str,
        download: Arc<dyn ReleaseDownload>,
        cache: PathBuf,
    ) -> Self {
        let url = format!("{}/nessa-{}", from.trim_end_matches('/'), pin.target);
        Self {
            pin,
            url,
            download,
            cache,
            opening: std::sync::Mutex::new(()),
        }
    }

    /// Its pin.
    pub(crate) fn pin(&self) -> &ReleasePin {
        &self.pin
    }

    /// The copy kept in the cache, if it is still the pinned bytes; one that
    /// is not is removed.
    fn kept(&self) -> Option<Build> {
        let path = self.cache.join(&self.pin.digest);
        let file = File::open(&path).ok()?;
        match sha256_of(&file) {
            Ok(digest) if digest == self.pin.digest => Some(Build { file, digest }),
            measured => {
                tracing::warn!(
                    path = %path.display(),
                    ?measured,
                    "a kept release build is not its pinned bytes; downloading it again"
                );
                let _ = fs::remove_file(&path);
                None
            }
        }
    }
}

/// Names each download's temporary file apart within this process.
static DOWNLOADS: AtomicU64 = AtomicU64::new(0);

impl BuildSource for ReleaseBuild {
    /// The pinned build, from the cache or downloaded now. Bytes that are
    /// not the pin's are handed back as they measured, never kept, so the
    /// installer refuses them and sends nothing.
    fn open(&self) -> io::Result<Build> {
        let _opening = self
            .opening
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(mut kept) = self.kept() {
            kept.file.seek(SeekFrom::Start(0))?;
            return Ok(kept);
        }
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.cache)?;
        let part = self.cache.join(format!(
            ".download.{}.{}",
            std::process::id(),
            DOWNLOADS.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&part)?;
        let measured = self
            .download
            .fetch(&self.url, self.pin.bytes, &mut file)
            .and_then(|()| file.flush())
            .and_then(|()| sha256_of(&file));
        let kept = match &measured {
            Ok(digest) if *digest == self.pin.digest => {
                fs::rename(&part, self.cache.join(&self.pin.digest))
            }
            _ => fs::remove_file(&part),
        };
        if let Err(error) = kept {
            tracing::warn!(%error, "a downloaded release build could not be kept or removed");
        }
        let digest = measured?;
        file.seek(SeekFrom::Start(0))?;
        Ok(Build { file, digest })
    }
}

/// How long to wait for the release host to answer at all.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// The longest one build may take to arrive: a `nessa` is tens of
/// megabytes, which a slow link takes minutes over. The installer's own
/// upload bound comes after it.
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// How many hops a redirect chain may take: a release download is answered
/// by a redirect to where its bytes are stored.
const REDIRECT_LIMIT: usize = 10;

/// What is moved between the socket and the file at a time.
const TRANSFER_CHUNK: usize = 64 * 1024;

/// Release builds fetched over HTTPS. Blocking, so it is only called from
/// a blocking thread (the installer opens a build in `spawn_blocking`).
#[derive(Default)]
pub(crate) struct HttpsReleases {
    client: OnceLock<Result<Client, String>>,
}

impl HttpsReleases {
    /// Its client, built the first time a build is fetched: one that stays
    /// on HTTPS, follows a bounded redirect chain and verifies certificates.
    fn client(&self) -> io::Result<&Client> {
        self.client
            .get_or_init(|| {
                install_tls_backend();
                Client::builder()
                    .connect_timeout(CONNECT_TIMEOUT)
                    .timeout(TRANSFER_TIMEOUT)
                    .https_only(true)
                    .redirect(Policy::limited(REDIRECT_LIMIT))
                    .user_agent(concat!("nessa/", env!("CARGO_PKG_VERSION")))
                    .build()
                    .map_err(|error| error.to_string())
            })
            .as_ref()
            .map_err(|error| io::Error::other(format!("no https client: {error}")))
    }
}

impl ReleaseDownload for HttpsReleases {
    fn fetch(&self, url: &str, at_most: u64, into: &mut File) -> io::Result<()> {
        let mut response =
            self.client()?.get(url).send().map_err(|error| {
                io::Error::other(format!("{url} could not be reached: {error}"))
            })?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(io::Error::other(format!("{url} answered {status}")));
        }
        store(&mut response, into, at_most)
    }
}

/// Move a body into `into`, failing on the read that takes it past `limit`
/// bytes: the pin's own length, not what the server says it sends.
pub(super) fn store(body: &mut impl Read, into: &mut impl Write, limit: u64) -> io::Result<()> {
    let mut buffer = vec![0u8; TRANSFER_CHUNK];
    let mut written: u64 = 0;
    loop {
        let read = body.read(&mut buffer)?;
        if read == 0 {
            return Ok(());
        }
        written += read as u64;
        if written > limit {
            return Err(io::Error::other(format!(
                "longer than its pinned {limit} bytes"
            )));
        }
        into.write_all(&buffer[..read])?;
    }
}

#[cfg(test)]
#[path = "../../../../tests/conversation/ssh_release.rs"]
mod tests;
