//! The release builds a release pins for other platforms (#734): how the
//! pins read, and how one is downloaded, measured and kept.
use super::*;
use sha2::{Digest, Sha256};
use std::sync::Mutex;

const LINUX: &str = "x86_64-unknown-linux-gnu";

fn digest_of(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn the_pins_a_release_is_compiled_with_read_as_each_targets_build() {
    let (a, b) = ("a".repeat(64), "b".repeat(64));
    let read = pins(&format!("aarch64-apple-darwin={a}:1024,{LINUX}={b}:2048")).unwrap();
    assert_eq!(
        read,
        [
            ReleasePin {
                target: "aarch64-apple-darwin".into(),
                platform: Platform::of_target("aarch64-apple-darwin").unwrap(),
                digest: a.clone(),
                bytes: 1024,
            },
            ReleasePin {
                target: LINUX.into(),
                platform: Platform::of_target(LINUX).unwrap(),
                digest: b,
                bytes: 2048,
            },
        ]
    );
}

#[test]
fn pins_that_do_not_read_are_refused_whole() {
    let a = "a".repeat(64);
    for spec in [
        String::new(),
        LINUX.to_owned(),
        format!("{LINUX}={a}"),
        format!("aarch64-unknown-linux-gnu={a}:1"),
        format!("{LINUX}={a}:1,{LINUX}={a}:1"),
        format!("{LINUX}={}:1", "A".repeat(64)),
        format!("{LINUX}={}:1", "a".repeat(63)),
        format!("{LINUX}={a}:0"),
        format!("{LINUX}={a}:-1"),
        format!("{LINUX}={a}:many"),
        format!("{LINUX}={a}:1,"),
    ] {
        assert!(pins(&spec).is_err(), "{spec:?}");
    }
}

/// Serves `bytes` for every fetch, or fails it, and counts the fetches.
struct Served {
    bytes: Result<Vec<u8>, String>,
    urls: Mutex<Vec<String>>,
}

impl Served {
    fn new(bytes: Result<&[u8], &str>) -> Arc<Self> {
        Arc::new(Self {
            bytes: bytes.map(<[u8]>::to_vec).map_err(str::to_owned),
            urls: Mutex::new(Vec::new()),
        })
    }
    fn urls(&self) -> Vec<String> {
        self.urls.lock().unwrap().clone()
    }
}

impl ReleaseDownload for Served {
    fn fetch(&self, url: &str, at_most: u64, into: &mut File) -> io::Result<()> {
        self.urls.lock().unwrap().push(url.into());
        match &self.bytes {
            Ok(bytes) => store(&mut bytes.as_slice(), into, at_most),
            Err(error) => Err(io::Error::other(error.clone())),
        }
    }
}

fn pinned(bytes: &[u8]) -> ReleasePin {
    ReleasePin {
        target: LINUX.into(),
        platform: Platform::of_target(LINUX).unwrap(),
        digest: digest_of(bytes),
        bytes: bytes.len() as u64,
    }
}

fn read(build: Build) -> Vec<u8> {
    let mut file = build.file;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    bytes
}

/// Downloaded once from the release, measured, kept by its digest, and
/// read from the cache after that.
#[test]
fn a_release_build_is_downloaded_once_and_kept_by_its_digest() {
    let cache = tempfile::tempdir().unwrap();
    let bytes = b"nessa for linux";
    let served = Served::new(Ok(bytes));
    let build = ReleaseBuild::new(
        pinned(bytes),
        "https://example.test/releases/download/v1.2.3/",
        served.clone(),
        cache.path().join("host-builds"),
    );
    for _ in 0..2 {
        let opened = build.open().unwrap();
        assert_eq!(opened.digest, digest_of(bytes));
        assert_eq!(read(opened), bytes);
    }
    assert_eq!(
        served.urls(),
        [format!(
            "https://example.test/releases/download/v1.2.3/nessa-{LINUX}"
        )]
    );
    let kept: Vec<_> = std::fs::read_dir(cache.path().join("host-builds"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(kept, [digest_of(bytes)]);
}

/// A kept copy that is no longer the pinned bytes is not used: it is
/// downloaded again.
#[test]
fn a_kept_copy_that_changed_is_downloaded_again() {
    let cache = tempfile::tempdir().unwrap();
    let bytes = b"nessa for linux";
    let served = Served::new(Ok(bytes));
    let build = ReleaseBuild::new(
        pinned(bytes),
        "https://example.test",
        served.clone(),
        cache.path().into(),
    );
    build.open().unwrap();
    std::fs::write(cache.path().join(digest_of(bytes)), b"something else").unwrap();
    assert_eq!(read(build.open().unwrap()), bytes);
    assert_eq!(served.urls().len(), 2);
}

/// What the release serves that is not the pinned build is handed back as
/// it measured, so the installer refuses it, and nothing of it is kept.
#[test]
fn a_download_that_is_not_the_pin_is_measured_and_not_kept() {
    let cache = tempfile::tempdir().unwrap();
    let pin = pinned(b"nessa for linux");
    let served = Served::new(Ok(b"nessa for linuX"));
    let build = ReleaseBuild::new(
        pin.clone(),
        "https://example.test",
        served,
        cache.path().into(),
    );
    let opened = build.open().unwrap();
    assert_eq!(opened.digest, digest_of(b"nessa for linuX"));
    assert_ne!(opened.digest, pin.digest);
    assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 0);
}

/// Longer than its pin, or not downloaded at all: an error, nothing kept.
#[test]
fn a_download_longer_than_its_pin_or_failed_is_an_error_and_not_kept() {
    let pin = pinned(b"nessa");
    for served in [
        Served::new(Ok(b"nessa and more")),
        Served::new(Err("unreachable")),
    ] {
        let cache = tempfile::tempdir().unwrap();
        let build = ReleaseBuild::new(
            pin.clone(),
            "https://example.test",
            served,
            cache.path().into(),
        );
        assert!(build.open().is_err());
        assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 0);
    }
}

/// The body is refused on the read that crosses the bound, and what fits
/// is written whole.
#[test]
fn a_body_is_stored_up_to_its_bound() {
    let mut into = Vec::new();
    store(&mut &b"12345"[..], &mut into, 5).unwrap();
    assert_eq!(into, b"12345");
    assert!(store(&mut &b"123456"[..], &mut Vec::new(), 5).is_err());
}
