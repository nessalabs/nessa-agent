//! Where a conversation runs, as the file the gateway keeps for it: written
//! whole, read back exactly, refused when this build cannot read it, and
//! gone with the conversation.
use super::{FileBindings, FilePlacements};
use crate::conversation::application::{
    ConversationBindings, ConversationPlacements, PlacementError, WorkerBinding,
};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::agent_execution::leases::PeerEnvironment;
use nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId;

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

/// The pool is placed by its own name and read back as it.
#[tokio::test]
async fn a_placement_on_any_worker_is_read_back_as_written() {
    let (_root, placements) = store();
    placements
        .place(&conversation(), Some("peer:any"))
        .await
        .unwrap();
    assert_eq!(
        placements.placement(&conversation()).await,
        Ok(Some("peer:any".into()))
    );
}

fn bindings() -> (tempfile::TempDir, FileBindings) {
    let root = tempfile::tempdir().unwrap();
    let bindings = FileBindings::new(root.path().join("bindings")).unwrap();
    (root, bindings)
}

fn binding_file(root: &tempfile::TempDir) -> std::path::PathBuf {
    root.path()
        .join("bindings")
        .join(format!("{CONVERSATION}.json"))
}

/// Which worker holds a conversation, and the context it opened there, are
/// read back as written; a later binding replaces it whole, and erasing it
/// twice is quiet.
#[tokio::test]
async fn a_binding_is_read_back_as_written_replaced_whole_and_erased() {
    let (root, bindings) = bindings();
    assert_eq!(bindings.binding(&conversation()).await, Ok(None));
    let first = WorkerBinding {
        worker: PeerEnvironment::new([1; 32]),
        context: Some(ExecutionSessionId::new("session-on-one").unwrap()),
    };
    bindings.bind(&conversation(), &first).await.unwrap();
    assert_eq!(bindings.binding(&conversation()).await, Ok(Some(first)));
    let written = std::fs::read_to_string(binding_file(&root)).unwrap();
    assert_eq!(
        written,
        format!(
            "{{\"schemaVersion\":1,\"worker\":\"peer:{}\",\"context\":\"session-on-one\"}}\n",
            "01".repeat(32)
        )
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(binding_file(&root))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "only its owner reads it");
    }
    let moved = WorkerBinding {
        worker: PeerEnvironment::new([2; 32]),
        context: None,
    };
    bindings.bind(&conversation(), &moved).await.unwrap();
    assert_eq!(bindings.binding(&conversation()).await, Ok(Some(moved)));
    assert!(!std::fs::read_to_string(binding_file(&root))
        .unwrap()
        .contains("context"));
    bindings.erase(&conversation()).await.unwrap();
    bindings.erase(&conversation()).await.unwrap();
    assert_eq!(bindings.binding(&conversation()).await, Ok(None));
}

#[tokio::test]
async fn a_binding_this_build_cannot_read_is_refused() {
    let worker = format!("peer:{}", "ab".repeat(32));
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
        let (root, bindings) = bindings();
        std::fs::write(binding_file(&root), &body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(binding_file(&root), std::fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        assert_eq!(
            bindings.binding(&conversation()).await,
            Err(PlacementError::Unreadable),
            "{body}"
        );
    }
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

/// A binding is replaced while others read it: a read that opens the file
/// being replaced reads the replacement, never takes it for one this build
/// cannot read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_binding_read_while_it_is_replaced_reads_one_whole_binding() {
    let (_root, bindings) = bindings();
    let bindings = std::sync::Arc::new(bindings);
    let binding = |byte: u8| WorkerBinding {
        worker: PeerEnvironment::new([byte; 32]),
        context: None,
    };
    bindings.bind(&conversation(), &binding(0)).await.unwrap();
    let writer = {
        let bindings = bindings.clone();
        tokio::spawn(async move {
            for round in 0..400u16 {
                let byte = u8::try_from(round % 2).unwrap();
                bindings
                    .bind(&conversation(), &binding(byte))
                    .await
                    .unwrap();
            }
        })
    };
    while !writer.is_finished() {
        let read = bindings.binding(&conversation()).await;
        assert!(
            matches!(&read, Ok(Some(read)) if *read == binding(0) || *read == binding(1)),
            "{read:?}"
        );
    }
    writer.await.unwrap();
}
