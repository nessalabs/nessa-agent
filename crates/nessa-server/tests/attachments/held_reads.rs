//! `HeldReads` over the real store: a conversation reads back exactly the
//! files it keeps, by range, and nothing once it lets one go.
use super::*;
use crate::attachments::application::{ArtifactBytes, HeldReadError, HeldReads};
use crate::attachments_test_support::{organization as organization_of, FixedOwnership};

/// Reads over `store`, for conversations the organization `org`'s `owner`
/// owns: both test conversations.
fn reads(store: LocalAttachmentStore) -> (Arc<LocalAttachmentStore>, HeldReads) {
    let store = Arc::new(store);
    let ownership = Arc::new(FixedOwnership::default());
    ownership.give(CONVERSATION, "org", "owner");
    ownership.give(OTHER_CONVERSATION, "org", "owner");
    (store.clone(), HeldReads::new(store, ownership))
}

/// `reads.read` as the owner of `hold`'s conversation.
async fn read(
    reads: &HeldReads,
    hold: &Hold,
    offset: u64,
    max_bytes: usize,
) -> Result<ArtifactBytes, HeldReadError> {
    reads
        .read(
            hold.organization_id(),
            &principal("owner"),
            hold.conversation_id(),
            hold.stored(),
            offset,
            max_bytes,
        )
        .await
}

#[tokio::test]
async fn a_held_file_reads_back_whole_by_ranges() {
    let root = tempfile::tempdir().unwrap();
    let (store, reads) = reads(open(root.path()));
    let bytes: Vec<_> = (0..HeldReads::MAX_RANGE_BYTES * 2 + 7)
        .map(|i| (i % 253) as u8)
        .collect();
    let hold = upload(&store, CONVERSATION, &bytes).await;
    let mut assembled = Vec::new();
    while (assembled.len() as u64) < hold.stored().size() {
        let range = read(
            &reads,
            &hold,
            assembled.len() as u64,
            HeldReads::MAX_RANGE_BYTES,
        )
        .await
        .unwrap();
        assert!(range.as_bytes().len() <= HeldReads::MAX_RANGE_BYTES);
        assembled.extend_from_slice(range.as_bytes());
    }
    assert_eq!(assembled, bytes);
}

#[tokio::test]
async fn a_range_outside_the_file_or_its_bound_is_invalid_and_reads_nothing() {
    let root = tempfile::tempdir().unwrap();
    let (store, reads) = reads(open(root.path()));
    let hold = upload(&store, CONVERSATION, b"twelve bytes").await;
    *store.source.after_open.lock().unwrap() =
        Some(Box::new(|| panic!("an invalid range opened bytes")));
    for (offset, max) in [
        (0, 0),
        (0, HeldReads::MAX_RANGE_BYTES + 1),
        (hold.stored().size(), 1),
        (u64::MAX, 1),
    ] {
        assert_eq!(
            read(&reads, &hold, offset, max).await,
            Err(HeldReadError::InvalidRequest)
        );
    }
}

#[tokio::test]
async fn only_the_owners_conversation_that_keeps_exactly_that_file_reads_it() {
    let root = tempfile::tempdir().unwrap();
    let (store, reads) = reads(open(root.path()));
    let hold = upload(&store, CONVERSATION, b"kept by one conversation").await;
    let read_as =
        |organization: &str, principal_id: &str, conversation_id: &str, stored: Attachment| {
            let reads = reads.clone();
            let (organization, principal_id, conversation_id) = (
                organization_of(organization),
                principal(principal_id),
                conversation(conversation_id),
            );
            async move {
                reads
                    .read(
                        &organization,
                        &principal_id,
                        &conversation_id,
                        &stored,
                        0,
                        4,
                    )
                    .await
                    .map(|bytes| bytes.as_bytes().to_vec())
            }
        };
    assert_eq!(
        read_as("org", "owner", CONVERSATION, hold.stored().clone()).await,
        Ok(b"kept".to_vec())
    );
    // Not the caller's conversation: the same answer as one that is not there.
    for (organization, principal_id) in [("org", "other"), ("other", "owner")] {
        assert_eq!(
            read_as(
                organization,
                principal_id,
                CONVERSATION,
                hold.stored().clone()
            )
            .await,
            Err(HeldReadError::ConversationNotFound)
        );
    }
    // The owner's other conversation, the same bytes declared as another
    // type, and a digest it never held: none of them keeps this file.
    for (conversation_id, stored) in [
        (OTHER_CONVERSATION, hold.stored().clone()),
        (
            CONVERSATION,
            attachment(b"kept by one conversation", "image/png"),
        ),
        (CONVERSATION, attachment(b"never uploaded", PDF)),
    ] {
        assert_eq!(
            read_as("org", "owner", conversation_id, stored).await,
            Err(HeldReadError::NotHeld)
        );
    }
}

#[tokio::test]
async fn a_pending_hold_is_not_read() {
    let root = tempfile::tempdir().unwrap();
    let (store, reads) = reads(open(root.path()));
    let hold = hold_for("org", CONVERSATION, b"pending", b"pending");
    let _claim = claim(&store, &hold, b"pending").await;
    assert_eq!(read(&reads, &hold, 0, 1).await, Err(HeldReadError::NotHeld));
}

#[tokio::test]
async fn a_release_between_two_ranges_stops_the_read() {
    let root = tempfile::tempdir().unwrap();
    let (store, reads) = reads(open(root.path()));
    let bytes = vec![7; HeldReads::MAX_RANGE_BYTES + 1];
    let hold = upload(&store, CONVERSATION, &bytes).await;
    let first = read(&reads, &hold, 0, HeldReads::MAX_RANGE_BYTES)
        .await
        .unwrap();
    assert_eq!(first.as_bytes().len(), HeldReads::MAX_RANGE_BYTES);
    store
        .release(
            hold.organization_id(),
            hold.conversation_id(),
            &release_evidence(),
        )
        .await
        .unwrap();
    assert_eq!(
        read(
            &reads,
            &hold,
            HeldReads::MAX_RANGE_BYTES as u64,
            HeldReads::MAX_RANGE_BYTES
        )
        .await,
        Err(HeldReadError::NotHeld)
    );
}

#[tokio::test(start_paused = true)]
async fn a_source_kept_busy_past_the_wait_answers_busy() {
    let root = tempfile::tempdir().unwrap();
    let (store, reads) = reads(open(root.path()));
    let hold = upload(&store, CONVERSATION, b"busy").await;
    // Hold the source's one slot inside a read until the test lets go.
    let (entered, entered_rx) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel::<()>();
    *store.source.after_open.lock().unwrap() = Some(Box::new(move || {
        entered.send(()).unwrap();
        release_rx.recv().unwrap();
    }));
    let blocked = tokio::spawn({
        let reads = reads.clone();
        let hold = hold.clone();
        async move { read(&reads, &hold, 0, 1).await }
    });
    tokio::task::spawn_blocking(move || entered_rx.recv().unwrap())
        .await
        .unwrap();
    assert_eq!(read(&reads, &hold, 0, 1).await, Err(HeldReadError::Busy));
    release.send(()).unwrap();
    assert_eq!(blocked.await.unwrap().unwrap().as_bytes(), b"b");
}
