//! Earlier builds' place records: a conversation with none, or one saying
//! here, runs here; one naming another machine is refused, never run here;
//! one this build cannot read refuses that conversation alone; all of them
//! go with the conversation.
use super::FilePlaces;
use crate::application::{ConversationPlaces, PlacementError};
use nessa_protocol::conversation::domain::ConversationId;
use std::path::PathBuf;

const CONVERSATION: &str = "5c0a9f2e-1d3b-4c7a-8e6f-2b9d4a1c7e30";

fn conversation() -> ConversationId {
    ConversationId::new(CONVERSATION).unwrap()
}

fn store() -> (tempfile::TempDir, FilePlaces) {
    let root = tempfile::tempdir().unwrap();
    let places = FilePlaces::new(root.path());
    (root, places)
}

/// The conversation's file in `directory` under the conversations root.
fn file(root: &tempfile::TempDir, directory: &str) -> PathBuf {
    root.path()
        .join(directory)
        .join(format!("{CONVERSATION}.json"))
}

/// Write `body` as the conversation's file in `directory`, private, as an
/// earlier build wrote it.
fn put(root: &tempfile::TempDir, directory: &str, body: &str) {
    use std::io::Write;
    let folder = root.path().join(directory);
    nessa_local_storage::create_directory(&folder).unwrap();
    let mut temporary = nessa_local_storage::PrivateTempFile::new_in(&folder).unwrap();
    temporary.as_file_mut().write_all(body.as_bytes()).unwrap();
    temporary.persist(&file(root, directory)).unwrap();
}

#[tokio::test]
async fn a_conversation_with_no_record_runs_here() {
    let (_root, places) = store();
    assert_eq!(places.read(&conversation()).await, Ok(()));
}

#[tokio::test]
async fn a_record_saying_here_runs_here() {
    let (root, places) = store();
    put(&root, "places", r#"{"schemaVersion":1,"placed":"here"}"#);
    assert_eq!(places.read(&conversation()).await, Ok(()));
}

#[tokio::test]
async fn a_record_naming_another_machine_is_refused_whatever_it_names() {
    for placed in [
        r#"{"schemaVersion":1,"placed":"devbox"}"#,
        r#"{"schemaVersion":1,"placed":"boat:thread"}"#,
        r#"{"schemaVersion":1,"placed":"peer:any","worker":"peer:AAAA","context":"c"}"#,
    ] {
        let (root, places) = store();
        put(&root, "places", placed);
        assert_eq!(
            places.read(&conversation()).await,
            Err(PlacementError::Elsewhere),
            "{placed}"
        );
    }
}

#[tokio::test]
async fn an_earlier_builds_placement_file_is_refused() {
    let (root, places) = store();
    put(
        &root,
        "placements",
        r#"{"schemaVersion":1,"host":"devbox"}"#,
    );
    assert_eq!(
        places.read(&conversation()).await,
        Err(PlacementError::Elsewhere)
    );
}

#[tokio::test]
async fn a_record_this_build_cannot_read_refuses_that_conversation() {
    for body in [
        "not json",
        r#"{"schemaVersion":2,"placed":"here"}"#,
        r#"{"schemaVersion":1,"placed":""}"#,
        r#"{"schemaVersion":1,"placed":"here","later":true}"#,
    ] {
        let (root, places) = store();
        put(&root, "places", body);
        assert_eq!(
            places.read(&conversation()).await,
            Err(PlacementError::Unreadable),
            "{body}"
        );
    }
}

#[tokio::test]
async fn erasing_removes_every_record_and_is_idempotent() {
    let (root, places) = store();
    put(&root, "places", r#"{"schemaVersion":1,"placed":"devbox"}"#);
    put(
        &root,
        "placements",
        r#"{"schemaVersion":1,"host":"devbox"}"#,
    );
    put(
        &root,
        "bindings",
        r#"{"schemaVersion":1,"worker":"peer:AAAA"}"#,
    );
    places.erase(&conversation()).await.unwrap();
    for directory in ["places", "placements", "bindings"] {
        assert!(!file(&root, directory).exists(), "{directory}");
    }
    places.erase(&conversation()).await.unwrap();
    assert_eq!(places.read(&conversation()).await, Ok(()));
}
