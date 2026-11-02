//! What a paired host keeps of a build its gateway sends: only every byte
//! it was told of, measured, and a copy that says the protocol refused.
use super::*;
use nessa_protocol::build_update::{encode, part};
use tokio::io::AsyncWriteExt;

const OLD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn script(protocol: &str) -> Vec<u8> {
    format!("#!/bin/sh\necho {protocol}\n").into_bytes()
}

fn digest_of(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// A gateway that sends `frames`, then ends, as the host's fetch reads.
async fn fetched(
    frames: Vec<FromGateway>,
    protocol: &str,
    builds: &Path,
) -> Result<PathBuf, NotUpdated> {
    fetched_by(frames, protocol, builds, OLD).await
}

/// As [`fetched`], by the build whose SHA-256 is `this_build`.
async fn fetched_by(
    frames: Vec<FromGateway>,
    protocol: &str,
    builds: &Path,
    this_build: &str,
) -> Result<PathBuf, NotUpdated> {
    let (host, gateway) = tokio::io::duplex(1 << 20);
    let (_from_host, mut to_host) = tokio::io::split(gateway);
    let sending = tokio::spawn(async move {
        for frame in frames {
            to_host.write_all(&encode(&frame).unwrap()).await.unwrap();
        }
        to_host.shutdown().await.unwrap();
        // The host's ask is read by nobody: kept open until it is done.
        _from_host
    });
    let (input, output) = tokio::io::split(host);
    let fetched = fetch(input, output, protocol, builds, this_build).await;
    drop(sending.await.unwrap());
    fetched
}

fn sent(build: &[u8], protocol: &str) -> Vec<FromGateway> {
    let mut frames = vec![FromGateway::Update {
        digest: digest_of(build),
        size: build.len() as u64,
        protocol: protocol.into(),
    }];
    frames.extend(build.chunks(7).map(part));
    frames.push(FromGateway::UpdateEnd);
    frames
}

/// What is left in `builds` besides `current`: nothing half-written.
fn left(builds: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(builds)
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_build_that_says_the_protocol_is_kept_by_its_digest_in_place_of_this_one() {
    let home = tempfile::tempdir().unwrap();
    let builds = home.path().join("builds");
    let build = script("proto");
    let kept = fetched(sent(&build, "proto"), "proto", &builds)
        .await
        .unwrap();
    let digest = digest_of(&build);
    assert_eq!(kept, builds.join(&digest).join("nessa"));
    assert_eq!(fs::read(&kept).unwrap(), build);
    assert_eq!(left(&builds), ["current".to_owned(), digest.clone()]);
    assert_eq!(kept_for(&builds, OLD), Some(kept.clone()));
    // Another build of this machine (a newer image) runs itself, and the
    // copy, which is not the build it replaced, never runs another.
    assert_eq!(kept_for(&builds, &"b".repeat(64)), None);
    assert_eq!(kept_for(&builds, &digest), None);
    // Gone, it is not run.
    fs::remove_file(&kept).unwrap();
    assert_eq!(kept_for(&builds, OLD), None);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_build_kept_removes_every_earlier_one_and_whatever_a_cut_fetch_left() {
    let home = tempfile::tempdir().unwrap();
    let builds = home.path().join("builds");
    let earlier = "d".repeat(64);
    fs::create_dir_all(builds.join(&earlier)).unwrap();
    fs::write(builds.join(&earlier).join("nessa"), b"old").unwrap();
    fs::write(builds.join(".update-left"), b"half").unwrap();
    fs::write(builds.join(".current-left"), b"half").unwrap();
    fs::create_dir_all(builds.join("other")).unwrap();
    let build = script("proto");
    fetched(sent(&build, "proto"), "proto", &builds)
        .await
        .unwrap();
    assert_eq!(
        left(&builds),
        ["current".to_owned(), digest_of(&build), "other".to_owned()]
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_copy_that_fetches_another_hands_it_to_the_image_build_it_runs_for() {
    let home = tempfile::tempdir().unwrap();
    let builds = home.path().join("builds");
    // The image's build (OLD) fetches B; B, running in its place, fetches C.
    let b = script("one");
    fetched(sent(&b, "one"), "one", &builds).await.unwrap();
    let c = script("two");
    let kept = fetched_by(sent(&c, "two"), "two", &builds, &digest_of(&b))
        .await
        .unwrap();
    // The image started again runs C at once; B is gone; C runs itself.
    assert_eq!(kept_for(&builds, OLD), Some(kept));
    let mut expected = ["current".to_owned(), digest_of(&c)];
    expected.sort();
    assert_eq!(left(&builds), expected);
    assert_eq!(kept_for(&builds, &digest_of(&c)), None);
    assert_eq!(kept_for(&builds, &digest_of(&b)), None);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_copy_that_says_another_protocol_is_not_kept() {
    let home = tempfile::tempdir().unwrap();
    let builds = home.path().join("builds");
    let build = script("other");
    let refused = fetched(sent(&build, "proto"), "proto", &builds).await;
    assert!(matches!(
        refused,
        Err(NotUpdated::Unverified("the protocol it says"))
    ));
    assert!(left(&builds).is_empty(), "{:?}", left(&builds));
    assert_eq!(kept_for(&builds, OLD), None);
}

#[tokio::test]
async fn bytes_that_are_not_the_digest_said_are_not_kept() {
    let home = tempfile::tempdir().unwrap();
    let builds = home.path().join("builds");
    let build = script("proto");
    let mut frames = sent(&build, "proto");
    frames[0] = FromGateway::Update {
        digest: "c".repeat(64),
        size: build.len() as u64,
        protocol: "proto".into(),
    };
    let refused = fetched(frames, "proto", &builds).await;
    assert!(matches!(
        refused,
        Err(NotUpdated::Unverified("its SHA-256"))
    ));
    assert!(left(&builds).is_empty());
}

#[tokio::test]
async fn more_or_fewer_bytes_than_said_are_not_kept() {
    let home = tempfile::tempdir().unwrap();
    let builds = home.path().join("builds");
    let build = script("proto");
    for size in [build.len() as u64 - 1, build.len() as u64 + 1] {
        let mut frames = sent(&build, "proto");
        frames[0] = FromGateway::Update {
            digest: digest_of(&build),
            size,
            protocol: "proto".into(),
        };
        let refused = fetched(frames, "proto", &builds).await;
        assert!(
            matches!(refused, Err(NotUpdated::Unverified(_))),
            "{refused:?}"
        );
        assert!(left(&builds).is_empty());
    }
}

#[tokio::test]
async fn a_build_for_another_protocol_than_the_one_refused_is_not_taken() {
    let home = tempfile::tempdir().unwrap();
    let builds = home.path().join("builds");
    let build = script("proto");
    let refused = fetched(sent(&build, "proto"), "another", &builds).await;
    assert!(matches!(
        refused,
        Err(NotUpdated::Unverified(
            "another protocol than the one refused"
        ))
    ));
    assert!(left(&builds).is_empty());
}

#[tokio::test]
async fn a_build_larger_than_any_taken_is_not_started() {
    let home = tempfile::tempdir().unwrap();
    let builds = home.path().join("builds");
    let frames = vec![FromGateway::Update {
        digest: "c".repeat(64),
        size: MOST_UPDATE_BYTES + 1,
        protocol: "proto".into(),
    }];
    let refused = fetched(frames, "proto", &builds).await;
    assert!(matches!(refused, Err(NotUpdated::Unverified("its size"))));
    assert!(left(&builds).is_empty());
}

#[tokio::test]
async fn a_gateway_with_no_build_for_this_platform_says_why() {
    let home = tempfile::tempdir().unwrap();
    let builds = home.path().join("builds");
    let refused = fetched(
        vec![FromGateway::UpdateRefused {
            reason: "platform".into(),
        }],
        "proto",
        &builds,
    )
    .await;
    assert!(matches!(refused, Err(NotUpdated::Refused(reason)) if reason == "platform"));
}

#[tokio::test]
async fn a_send_cut_short_leaves_nothing() {
    let home = tempfile::tempdir().unwrap();
    let builds = home.path().join("builds");
    let build = script("proto");
    let mut frames = sent(&build, "proto");
    frames.truncate(2);
    let refused = fetched(frames, "proto", &builds).await;
    assert!(matches!(refused, Err(NotUpdated::Cut)));
    assert!(left(&builds).is_empty(), "{:?}", left(&builds));
}

#[test]
fn a_current_naming_no_digest_is_not_run() {
    let home = tempfile::tempdir().unwrap();
    let builds = home.path().join("builds");
    fs::create_dir_all(builds.join("..")).unwrap();
    fs::create_dir_all(&builds).unwrap();
    fs::create_dir_all(builds.join("x")).unwrap();
    fs::write(builds.join("x/nessa"), b"").unwrap();
    fs::write(builds.join("current"), format!("x {OLD}\n")).unwrap();
    assert_eq!(kept_for(&builds, OLD), None);
    fs::write(builds.join("current"), "").unwrap();
    assert_eq!(kept_for(&builds, OLD), None);
}
