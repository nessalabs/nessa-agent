//! Where a conversation runs, as the one file the gateway keeps for it:
//! written whole, read back exactly, refused when this build cannot read it,
//! and gone with the conversation. An earlier build's placement and binding
//! files are read as they were written until the record is written anew,
//! and written beside it for a build put back.
use super::FilePlaces;
use crate::conversation::application::{
    ConversationPlaces, PlaceRecord, PlacementError, WorkerBinding,
};
use crate::conversation::domain::Place;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::agent_execution::leases::PeerEnvironment;
use nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId;
use std::path::PathBuf;

const CONVERSATION: &str = "5c0a9f2e-1d3b-4c7a-8e6f-2b9d4a1c7e30";

fn conversation() -> ConversationId {
    ConversationId::new(CONVERSATION).unwrap()
}

fn named(name: &str) -> Place {
    Place::named(name).unwrap()
}

fn store() -> (tempfile::TempDir, FilePlaces) {
    let root = tempfile::tempdir().unwrap();
    let places = FilePlaces::new(root.path()).unwrap();
    (root, places)
}

/// The conversation's file in `directory` under the conversations root.
fn file(root: &tempfile::TempDir, directory: &str) -> PathBuf {
    root.path()
        .join(directory)
        .join(format!("{CONVERSATION}.json"))
}

/// Write `body` as the conversation's file in `directory`, private, as a
/// build writes it, so what is read is the content and not the file.
fn put(root: &tempfile::TempDir, directory: &str, body: &str) {
    use std::io::Write;
    let mut temporary =
        nessa_local_storage::PrivateTempFile::new_in(&root.path().join(directory)).unwrap();
    temporary.as_file_mut().write_all(body.as_bytes()).unwrap();
    temporary.persist(&file(root, directory)).unwrap();
}

fn binding(byte: u8, context: Option<&str>) -> WorkerBinding {
    WorkerBinding {
        worker: PeerEnvironment::new([byte; 32]),
        context: context.map(|context| ExecutionSessionId::new(context).unwrap()),
    }
}

#[tokio::test]
async fn a_conversation_with_no_record_runs_here() {
    let (_root, places) = store();
    assert_eq!(
        places.read(&conversation()).await,
        Ok(PlaceRecord::placed(Place::Here))
    );
}

/// Every place, here included, is written by its name, read back as the
/// same record, and kept private.
#[tokio::test]
async fn each_place_is_written_by_its_name_and_read_back_as_written() {
    let peer = format!("peer:{}", "ab".repeat(32));
    for (place, name) in [
        (Place::Here, "here"),
        (named("me@devbox"), "me@devbox"),
        (named(&peer), peer.as_str()),
        (Place::Pool, "peer:any"),
        (Place::CloudVm, "boat:thread"),
    ] {
        let (root, places) = store();
        let record = PlaceRecord::placed(place);
        places.write(&conversation(), &record).await.unwrap();
        assert_eq!(places.read(&conversation()).await, Ok(record));
        assert_eq!(
            std::fs::read_to_string(file(&root, "places")).unwrap(),
            format!("{{\"schemaVersion\":1,\"placed\":\"{name}\"}}\n")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(file(&root, "places"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "only its owner reads it");
        }
    }
}

/// Which worker holds a pool conversation, and the context it opened there,
/// are read back as written; a later record replaces it whole.
#[tokio::test]
async fn a_pool_conversations_worker_is_read_back_as_written_and_replaced_whole() {
    let (root, places) = store();
    let first = PlaceRecord::in_pool(binding(1, Some("session-on-one")));
    places.write(&conversation(), &first).await.unwrap();
    assert_eq!(places.read(&conversation()).await, Ok(first.clone()));
    assert_eq!(first.now(), Place::Peer(PeerEnvironment::new([1; 32])));
    assert_eq!(
        std::fs::read_to_string(file(&root, "places")).unwrap(),
        format!(
            "{{\"schemaVersion\":1,\"placed\":\"peer:any\",\"worker\":\"peer:{}\",\"context\":\"session-on-one\"}}\n",
            "01".repeat(32)
        )
    );
    let moved = PlaceRecord::in_pool(binding(2, None));
    places.write(&conversation(), &moved).await.unwrap();
    assert_eq!(places.read(&conversation()).await, Ok(moved));
    assert!(!std::fs::read_to_string(file(&root, "places"))
        .unwrap()
        .contains("context"));
}

/// A conversation an earlier build placed is read from its placement and,
/// on the pool, its binding, exactly as that build read them.
#[tokio::test]
async fn an_earlier_builds_placement_and_binding_are_read_as_it_read_them() {
    let worker = format!("peer:{}", "01".repeat(32));
    let cases = [
        (None, None, PlaceRecord::placed(Place::Here)),
        (
            Some("me@devbox"),
            None,
            PlaceRecord::placed(named("me@devbox")),
        ),
        (
            Some("boat:thread"),
            None,
            PlaceRecord::placed(Place::CloudVm),
        ),
        (Some("peer:any"), None, PlaceRecord::placed(Place::Pool)),
        (
            Some("peer:any"),
            Some(format!(
                "{{\"schemaVersion\":1,\"worker\":\"{worker}\",\"context\":\"session-on-one\"}}"
            )),
            PlaceRecord::in_pool(binding(1, Some("session-on-one"))),
        ),
        (
            Some("peer:any"),
            Some(format!("{{\"schemaVersion\":1,\"worker\":\"{worker}\"}}")),
            PlaceRecord::in_pool(binding(1, None)),
        ),
        // A binding beside a placement that is not the pool was never read.
        (
            Some("me@devbox"),
            Some(format!("{{\"schemaVersion\":1,\"worker\":\"{worker}\"}}")),
            PlaceRecord::placed(named("me@devbox")),
        ),
    ];
    for (host, bound, expected) in cases {
        let (root, places) = store();
        if let Some(host) = host {
            put(
                &root,
                "placements",
                &format!("{{\"schemaVersion\":1,\"host\":\"{host}\"}}"),
            );
        }
        if let Some(bound) = &bound {
            put(&root, "bindings", bound);
        }
        assert_eq!(
            places.read(&conversation()).await,
            Ok(expected),
            "{host:?} {bound:?}"
        );
    }
}

/// The first change writes the new record, which wins from then on, and the
/// earlier build's files as that build would have; erasing removes all of
/// them.
#[tokio::test]
async fn the_first_change_writes_the_new_record_and_the_earlier_files() {
    let (root, places) = store();
    put(
        &root,
        "placements",
        "{\"schemaVersion\":1,\"host\":\"peer:any\"}",
    );
    let old = format!(
        "{{\"schemaVersion\":1,\"worker\":\"peer:{}\",\"context\":\"old\"}}",
        "01".repeat(32)
    );
    put(&root, "bindings", &old);
    let moved = PlaceRecord::in_pool(binding(2, None));
    places.write(&conversation(), &moved).await.unwrap();
    assert_eq!(places.read(&conversation()).await, Ok(moved));
    assert_eq!(
        std::fs::read_to_string(file(&root, "bindings")).unwrap(),
        format!(
            "{{\"schemaVersion\":1,\"worker\":\"peer:{}\"}}\n",
            "02".repeat(32)
        ),
        "a build put back follows the move"
    );
    assert_eq!(
        std::fs::read_to_string(file(&root, "placements")).unwrap(),
        "{\"schemaVersion\":1,\"host\":\"peer:any\"}\n"
    );
    places.erase(&conversation()).await.unwrap();
    places.erase(&conversation()).await.unwrap();
    for directory in ["places", "placements", "bindings"] {
        assert!(!file(&root, directory).exists(), "{directory} is erased");
    }
    assert_eq!(
        places.read(&conversation()).await,
        Ok(PlaceRecord::placed(Place::Here))
    );
}

/// A build put back moves a pool conversation in its binding alone; coming
/// back, that move wins over the record this build wrote before it. A
/// record placed anywhere else ignores a binding, as earlier builds did.
#[tokio::test]
async fn a_move_made_by_a_build_put_back_wins_over_the_record() {
    let (root, places) = store();
    let first = PlaceRecord::in_pool(binding(1, Some("first")));
    places.write(&conversation(), &first).await.unwrap();
    put(
        &root,
        "bindings",
        &format!(
            "{{\"schemaVersion\":1,\"worker\":\"peer:{}\",\"context\":\"later\"}}",
            "02".repeat(32)
        ),
    );
    assert_eq!(
        places.read(&conversation()).await,
        Ok(PlaceRecord::in_pool(binding(2, Some("later"))))
    );
    let devbox = PlaceRecord::placed(named("devbox"));
    places.write(&conversation(), &devbox).await.unwrap();
    put(
        &root,
        "bindings",
        &format!(
            "{{\"schemaVersion\":1,\"worker\":\"peer:{}\"}}",
            "02".repeat(32)
        ),
    );
    assert_eq!(places.read(&conversation()).await, Ok(devbox));
}

#[tokio::test]
async fn writing_here_replaces_what_a_failed_creation_left_and_erasing_twice_is_quiet() {
    let (root, places) = store();
    let devbox = PlaceRecord::placed(named("devbox"));
    places.write(&conversation(), &devbox).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(file(&root, "placements")).unwrap(),
        "{\"schemaVersion\":1,\"host\":\"devbox\"}\n",
        "a build put back finds the host"
    );
    let here = PlaceRecord::placed(Place::Here);
    places.write(&conversation(), &here).await.unwrap();
    assert_eq!(places.read(&conversation()).await, Ok(here.clone()));
    assert!(
        !file(&root, "placements").exists(),
        "a build put back reads no placement as here"
    );
    places.write(&conversation(), &devbox).await.unwrap();
    places.erase(&conversation()).await.unwrap();
    places.erase(&conversation()).await.unwrap();
    assert_eq!(places.read(&conversation()).await, Ok(here));
}

#[tokio::test]
async fn a_record_this_build_cannot_read_is_refused_never_taken_for_here() {
    let worker = format!("peer:{}", "ab".repeat(32));
    for body in [
        "not json".to_owned(),
        "{\"schemaVersion\":2,\"placed\":\"devbox\"}".to_owned(),
        "{\"schemaVersion\":1,\"placed\":\"devbox\",\"port\":22}".to_owned(),
        "{\"schemaVersion\":1,\"placed\":\"-oProxyCommand=x\"}".to_owned(),
        "{\"schemaVersion\":1,\"placed\":\"peer:\"}".to_owned(),
        "{\"schemaVersion\":1,\"placed\":\"peer:zz\"}".to_owned(),
        "{\"schemaVersion\":1,\"host\":\"devbox\"}".to_owned(),
        // A worker or a context belongs to a pool conversation alone.
        format!("{{\"schemaVersion\":1,\"placed\":\"devbox\",\"worker\":\"{worker}\"}}"),
        "{\"schemaVersion\":1,\"placed\":\"peer:any\",\"context\":\"one\"}".to_owned(),
        "{\"schemaVersion\":1,\"placed\":\"peer:any\",\"worker\":\"devbox\"}".to_owned(),
        "{\"schemaVersion\":1,\"placed\":\"peer:any\",\"worker\":\"peer:any\"}".to_owned(),
        format!("{{\"schemaVersion\":1,\"placed\":\"peer:any\",\"worker\":\"{worker}\",\"context\":\" \"}}"),
        format!(
            "{{\"schemaVersion\":1,\"placed\":\"peer:any\",\"worker\":\"{worker}\",\"context\":\"{}\"}}",
            "a".repeat(300)
        ),
        format!("{{\"schemaVersion\":1,\"placed\":\"{}\"}}", "a".repeat(2048)),
    ] {
        let (root, places) = store();
        put(&root, "places", &body);
        assert_eq!(
            places.read(&conversation()).await,
            Err(PlacementError::Unreadable),
            "{body}"
        );
    }
}

#[tokio::test]
async fn an_earlier_placement_or_binding_this_build_cannot_read_is_refused() {
    let worker = format!("peer:{}", "ab".repeat(32));
    for body in [
        "not json".to_owned(),
        "{\"schemaVersion\":2,\"host\":\"devbox\"}".to_owned(),
        "{\"schemaVersion\":1,\"host\":\"devbox\",\"port\":22}".to_owned(),
        "{\"schemaVersion\":1,\"host\":\"-oProxyCommand=x\"}".to_owned(),
        "{\"schemaVersion\":1,\"host\":\"here\"}".to_owned(),
        "{\"schemaVersion\":1,\"host\":\"peer:\"}".to_owned(),
        "{\"schemaVersion\":1,\"host\":\"peer:zz\"}".to_owned(),
        format!("{{\"schemaVersion\":1,\"host\":\"{}\"}}", "a".repeat(2048)),
    ] {
        let (root, places) = store();
        put(&root, "placements", &body);
        assert_eq!(
            places.read(&conversation()).await,
            Err(PlacementError::Unreadable),
            "{body}"
        );
    }
    for body in [
        "not json".to_owned(),
        format!("{{\"schemaVersion\":2,\"worker\":\"{worker}\"}}"),
        format!("{{\"schemaVersion\":1,\"worker\":\"{worker}\",\"extra\":1}}"),
        "{\"schemaVersion\":1,\"worker\":\"devbox\"}".to_owned(),
        "{\"schemaVersion\":1,\"worker\":\"peer:any\"}".to_owned(),
        format!("{{\"schemaVersion\":1,\"worker\":\"{worker}\",\"context\":\" \"}}"),
        format!(
            "{{\"schemaVersion\":1,\"worker\":\"{worker}\",\"context\":\"{}\"}}",
            "a".repeat(300)
        ),
        format!(
            "{{\"schemaVersion\":1,\"worker\":\"{}\"}}",
            "a".repeat(2048)
        ),
    ] {
        let (root, places) = store();
        put(
            &root,
            "placements",
            "{\"schemaVersion\":1,\"host\":\"peer:any\"}",
        );
        put(&root, "bindings", &body);
        assert_eq!(
            places.read(&conversation()).await,
            Err(PlacementError::Unreadable),
            "{body}"
        );
    }
}

/// A record is replaced while others read it: a read that opens the file
/// being replaced reads the replacement, never takes it for one this build
/// cannot read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_record_read_while_it_is_replaced_reads_one_whole_record() {
    let (_root, places) = store();
    let places = std::sync::Arc::new(places);
    let record = |byte: u8| PlaceRecord::in_pool(binding(byte, None));
    places.write(&conversation(), &record(0)).await.unwrap();
    let writer = {
        let places = places.clone();
        tokio::spawn(async move {
            for round in 0..400u16 {
                let byte = u8::try_from(round % 2).unwrap();
                places.write(&conversation(), &record(byte)).await.unwrap();
            }
        })
    };
    while !writer.is_finished() {
        let read = places.read(&conversation()).await;
        assert!(
            matches!(&read, Ok(read) if *read == record(0) || *read == record(1)),
            "{read:?}"
        );
    }
    writer.await.unwrap();
}
