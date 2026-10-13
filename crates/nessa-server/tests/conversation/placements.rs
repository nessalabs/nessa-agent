//! Where a conversation runs, as the file the gateway keeps for it: written
//! whole, read back exactly, refused when this build cannot read it, and
//! gone with the conversation.
use super::FilePlacements;
use crate::conversation::application::{ConversationPlacements, PlacementError};
use nessa_protocol::conversation::domain::ConversationId;

const CONVERSATION: &str = "5c0a9f2e-1d3b-4c7a-8e6f-2b9d4a1c7e30";

fn conversation() -> ConversationId {
    ConversationId::new(CONVERSATION).unwrap()
}

fn store() -> (tempfile::TempDir, FilePlacements) {
    let root = tempfile::tempdir().unwrap();
    let placements = FilePlacements::new(root.path().join("placements")).unwrap();
    (root, placements)
}

fn file(root: &tempfile::TempDir) -> std::path::PathBuf {
    root.path()
        .join("placements")
        .join(format!("{CONVERSATION}.json"))
}

#[tokio::test]
async fn a_conversation_with_no_placement_runs_here() {
    let (_root, placements) = store();
    assert_eq!(placements.placement(&conversation()).await, Ok(None));
}

#[tokio::test]
async fn a_placement_is_read_back_as_written_and_is_private() {
    let (root, placements) = store();
    placements
        .place(&conversation(), Some("me@devbox"))
        .await
        .unwrap();
    assert_eq!(
        placements.placement(&conversation()).await,
        Ok(Some("me@devbox".into()))
    );
    let written = std::fs::read_to_string(file(&root)).unwrap();
    assert_eq!(written, "{\"schemaVersion\":1,\"host\":\"me@devbox\"}\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(file(&root)).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "only its owner reads it");
    }
}

/// A paired environment is placed by its `peer:` name and read back as it.
#[tokio::test]
async fn a_placement_on_a_paired_environment_is_read_back_as_written() {
    let (_root, placements) = store();
    let peer = format!("peer:{}", "ab".repeat(32));
    placements
        .place(&conversation(), Some(&peer))
        .await
        .unwrap();
    assert_eq!(placements.placement(&conversation()).await, Ok(Some(peer)));
}

#[tokio::test]
async fn placing_here_removes_what_a_failed_creation_left_and_erasing_twice_is_quiet() {
    let (_root, placements) = store();
    placements
        .place(&conversation(), Some("devbox"))
        .await
        .unwrap();
    placements.place(&conversation(), None).await.unwrap();
    assert_eq!(placements.placement(&conversation()).await, Ok(None));
    placements
        .place(&conversation(), Some("devbox"))
        .await
        .unwrap();
    placements.erase(&conversation()).await.unwrap();
    placements.erase(&conversation()).await.unwrap();
    assert_eq!(placements.placement(&conversation()).await, Ok(None));
}

#[tokio::test]
async fn a_placement_this_build_cannot_read_is_refused_never_taken_for_here() {
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
        let (root, placements) = store();
        std::fs::write(file(&root), &body).unwrap();
        #[cfg(unix)]
        {
            // Private, so what is refused is the content, not the file.
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(file(&root), std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert_eq!(
            placements.placement(&conversation()).await,
            Err(PlacementError::Unreadable),
            "{body}"
        );
    }
}
