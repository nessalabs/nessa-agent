//! A paired host on another protocol is told this gateway's, and is sent
//! this build for its platform when it asks; the host's own fetch takes it.
use super::super::install::{BuildSource, InstallTimings};
use super::*;
#[cfg(target_os = "linux")]
use crate::env_serve::infrastructure::{fetch, kept_for};
use nessa_protocol::build_update::{from_gateway, part_bytes};
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::sync::Mutex;
use tokio::io::AsyncReadExt;

/// A build that runs on Linux and says `protocol` when asked.
fn script(protocol: &str) -> Vec<u8> {
    format!("#!/bin/sh\necho {protocol}\n").into_bytes()
}

fn digest_of(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

struct Bytes(Vec<u8>);
impl BuildSource for Bytes {
    fn open(&self) -> io::Result<Build> {
        let mut file = tempfile::tempfile()?;
        file.write_all(&self.0)?;
        file.seek(SeekFrom::Start(0))?;
        Ok(Build {
            file,
            digest: digest_of(&self.0),
        })
    }
}

struct NoShell;
impl super::super::install::RemoteShell for NoShell {
    fn run<'a>(
        &'a self,
        _: &'a nessa_sdk::domain::agent_execution::leases::SshDestination,
        _: String,
        _: Option<File>,
    ) -> super::super::install::ShellFuture<'a> {
        panic!("a paired host's update ran a shell")
    }
}

#[derive(Default)]
struct Audit(Mutex<Vec<EnvironmentEvent>>);
impl EnvironmentAudit for Audit {
    fn record(&self, event: &EnvironmentEvent) -> io::Result<()> {
        self.0.lock().unwrap().push(event.clone());
        Ok(())
    }
}

fn linux() -> Platform {
    Platform {
        os: "linux".into(),
        arch: "x86_64".into(),
        libc: "gnu".into(),
    }
}

fn updates(build: Vec<u8>, protocol: &'static str) -> (PeerUpdates, Arc<Audit>) {
    let installer = HostInstaller::new(
        Arc::new(NoShell),
        Arc::new(Bytes(build)),
        linux(),
        protocol,
        InstallTimings::default(),
    );
    let audit = Arc::new(Audit::default());
    (
        PeerUpdates::new(Arc::new(installer), audit.clone()).within(Duration::from_secs(10)),
        audit,
    )
}

/// Every frame the host was sent.
async fn frames(mut from: tokio::io::DuplexStream) -> Vec<FromGateway> {
    let mut bytes = Vec::new();
    from.read_to_end(&mut bytes).await.unwrap();
    let mut reader =
        nessa_protocol::pairing::FrameReader::new(nessa_protocol::lease::MAX_FRAME_BYTES);
    let mut frames = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let buffer = reader.unfilled().unwrap();
        let take = buffer.len().min(bytes.len() - at);
        buffer[..take].copy_from_slice(&bytes[at..at + take]);
        at += take;
        if let Some(body) = reader.filled(take).unwrap() {
            frames.push(from_gateway(&body).expect("a frame of the exchange"));
        }
    }
    frames
}

#[tokio::test]
async fn a_host_on_another_protocol_is_told_this_one() {
    let (updates, audit) = updates(script("p"), "0123456789abcdef");
    let (mut to, from) = tokio::io::duplex(1 << 16);
    updates.refuse_version(&mut to).await;
    drop(to);
    assert_eq!(
        frames(from).await,
        [FromGateway::VersionRefused {
            protocol: "0123456789abcdef".into()
        }]
    );
    assert!(audit.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_host_that_asks_is_sent_every_byte_of_this_build_and_its_send_recorded() {
    // Larger than one part, so the bytes are joined back in order.
    let mut build = script("p");
    build.extend(std::iter::repeat_n(b'#', UPDATE_PART_BYTES * 2 + 17));
    let (updates, audit) = updates(build.clone(), "proto");
    let (mut to, from) = tokio::io::duplex(1 << 20);
    let reading = tokio::spawn(frames(from));
    updates.send_build("peer:ab", linux(), &mut to).await;
    drop(to);
    let frames = reading.await.unwrap();
    assert_eq!(
        frames[0],
        FromGateway::Update {
            digest: digest_of(&build),
            size: build.len() as u64,
            protocol: "proto".into()
        }
    );
    assert_eq!(frames.last(), Some(&FromGateway::UpdateEnd));
    let sent: Vec<u8> = frames[1..frames.len() - 1]
        .iter()
        .flat_map(|frame| match frame {
            FromGateway::UpdatePart { data } => part_bytes(data).unwrap(),
            other => panic!("not a part: {other:?}"),
        })
        .collect();
    assert_eq!(sent, build);
    assert_eq!(
        *audit.0.lock().unwrap(),
        [
            EnvironmentEvent::UpdateStarted {
                host: "peer:ab".into(),
                platform: "linux x86_64 gnu".into(),
                digest: digest_of(&build),
            },
            EnvironmentEvent::UpdateSent {
                host: "peer:ab".into(),
                digest: digest_of(&build),
            },
        ]
    );
}

#[tokio::test]
async fn a_host_no_build_runs_on_is_told_so_and_sent_nothing() {
    let (updates, audit) = updates(script("p"), "proto");
    let (mut to, from) = tokio::io::duplex(1 << 16);
    let mac = Platform {
        os: "macos".into(),
        arch: "aarch64".into(),
        libc: "other".into(),
    };
    updates.send_build("peer:ab", mac, &mut to).await;
    drop(to);
    assert_eq!(
        frames(from).await,
        [FromGateway::UpdateRefused {
            reason: "platform".into()
        }]
    );
    assert_eq!(
        *audit.0.lock().unwrap(),
        [EnvironmentEvent::UpdateRefused {
            host: "peer:ab".into(),
            platform: "macos aarch64 other".into(),
            reason: InstallRefusal::Platform,
            seen: Some("macos aarch64 other".into()),
        }]
    );
}

/// Both ends as they run: the host's fetch asks, this gateway sends, and
/// the host keeps the build where it runs it from.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_host_fetches_this_build_and_keeps_it_in_place_of_its_own() {
    let build = script("proto");
    let (updates, audit) = updates(build.clone(), "proto");
    let (host, gateway) = tokio::io::duplex(1 << 20);
    let (from_host, mut to_host) = tokio::io::split(gateway);
    let home = tempfile::tempdir().unwrap();
    let builds = home.path().join("builds");
    let old = "a".repeat(64);
    let sending = tokio::spawn(async move {
        let mut stream = crate::env_serve::application::FrameStream::new(from_host);
        let first = stream.next().await.unwrap().unwrap();
        let Some(nessa_protocol::build_update::FromEnvironment::UpdateWanted { os, arch, libc }) =
            nessa_protocol::build_update::from_environment(&first)
        else {
            panic!("the host did not ask for the build first");
        };
        updates
            .send_build("peer:ab", Platform { os, arch, libc }, &mut to_host)
            .await;
    });
    let (input, output) = tokio::io::split(host);
    let kept = fetch(input, output, "proto", &builds, &old).await.unwrap();
    sending.await.unwrap();
    assert_eq!(kept, builds.join(digest_of(&build)).join("nessa"));
    assert_eq!(std::fs::read(&kept).unwrap(), build);
    assert_eq!(kept_for(&builds, &old), Some(kept));
    assert!(matches!(
        audit.0.lock().unwrap().last(),
        Some(EnvironmentEvent::UpdateSent { .. })
    ));
}

#[tokio::test]
async fn a_host_that_asks_again_at_once_is_told_busy_and_another_host_is_not() {
    let (updates, audit) = updates(script("p"), "proto");
    let (mut to, from) = tokio::io::duplex(1 << 16);
    let reading = tokio::spawn(frames(from));
    updates.send_build("peer:ab", linux(), &mut to).await;
    drop(to);
    assert_eq!(reading.await.unwrap().last(), Some(&FromGateway::UpdateEnd));

    let (mut to, from) = tokio::io::duplex(1 << 16);
    updates.send_build("peer:ab", linux(), &mut to).await;
    drop(to);
    assert_eq!(
        frames(from).await,
        [FromGateway::UpdateRefused {
            reason: "busy".into()
        }]
    );

    let (mut to, from) = tokio::io::duplex(1 << 16);
    let reading = tokio::spawn(frames(from));
    updates.send_build("peer:cd", linux(), &mut to).await;
    drop(to);
    assert_eq!(reading.await.unwrap().last(), Some(&FromGateway::UpdateEnd));
    // The busy answer is not an update refused: nothing was opened for it.
    assert_eq!(audit.0.lock().unwrap().len(), 4);
}
