//! Bounded local range agreement and physical source ownership on real storage.
use super::*;
use crate::attachments::application::{
    ArtifactRange, ArtifactReadError, ArtifactState, AttachmentArtifacts,
};
use nessa_sync::replication::artifacts::MAX_CHUNK_BYTES;
use std::{future::poll_fn, sync::mpsc, task::Poll, time::Duration};

fn range(hold: &Hold, claim: &HoldClaim, offset: u64, limit: usize) -> ArtifactRange {
    ArtifactRange::new(
        ArtifactId::from_generation(claim.as_str()),
        1,
        hold.stored().clone(),
        offset,
        limit,
    )
}

#[tokio::test]
async fn ranges_are_exact_bounded_stored_bytes_and_not_original_upload_bytes() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let bytes: Vec<_> = (0..MAX_CHUNK_BYTES + 19).map(|i| (i % 251) as u8).collect();
    let hold = hold_for("org", CONVERSATION, b"original", &bytes);
    let claim = claim(&store, &hold, &bytes).await;
    store.confirm(&hold, &claim).await.unwrap();
    for (offset, limit) in [
        (0, MAX_CHUNK_BYTES),
        (MAX_CHUNK_BYTES as u64, MAX_CHUNK_BYTES),
        (17, 3),
    ] {
        let request = range(&hold, &claim, offset, limit);
        let read = store
            .chunk(hold.organization_id(), hold.conversation_id(), &request)
            .await
            .unwrap();
        let end = bytes.len().min(offset as usize + limit);
        assert_eq!(read.as_bytes(), &bytes[offset as usize..end]);
        assert!(read.as_bytes().len() <= MAX_CHUNK_BYTES);
    }
    // Creating and cancelling an unpolled request has admitted nothing.
    drop(store.shutdown());
    assert!(store
        .manifest(
            hold.organization_id(),
            hold.conversation_id(),
            range(&hold, &claim, 0, 1).id()
        )
        .await
        .is_ok());
    store.shutdown().await.unwrap();
    assert_eq!(
        store
            .chunk(
                hold.organization_id(),
                hold.conversation_id(),
                &range(&hold, &claim, 0, 1)
            )
            .await,
        Err(ArtifactReadError::Unavailable)
    );
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn invalid_foreign_pending_and_retired_ranges_do_not_read_content() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let hold = hold_for("org", CONVERSATION, b"original", b"stored");
    let claim = claim(&store, &hold, b"stored").await;
    let request = range(&hold, &claim, 0, 2);
    assert_eq!(
        store
            .chunk(hold.organization_id(), hold.conversation_id(), &request)
            .await,
        Err(ArtifactReadError::Missing)
    );
    store.confirm(&hold, &claim).await.unwrap();
    for (revision, offset, limit) in [
        (0, 0, 1),
        (1, 0, 0),
        (1, 0, MAX_CHUNK_BYTES + 1),
        (1, hold.stored().size(), 1),
        (1, u64::MAX, 1),
    ] {
        let request = ArtifactRange::new(
            request.id().clone(),
            revision,
            hold.stored().clone(),
            offset,
            limit,
        );
        assert_eq!(
            store
                .chunk(hold.organization_id(), hold.conversation_id(), &request)
                .await,
            Err(ArtifactReadError::InvalidRequest)
        );
    }
    for (revision, stored) in [(2, hold.stored().clone()), (1, hold.uploaded().clone())] {
        let wrong = ArtifactRange::new(request.id().clone(), revision, stored, 0, 1);
        assert_eq!(
            store
                .chunk(hold.organization_id(), hold.conversation_id(), &wrong)
                .await,
            Err(ArtifactReadError::Changed)
        );
    }
    assert_eq!(
        store
            .chunk(&organization("other"), hold.conversation_id(), &request)
            .await,
        Err(ArtifactReadError::Missing)
    );
    let other = range(&hold, &HoldClaim::new("another generation"), 0, 1);
    assert_eq!(
        store
            .chunk(hold.organization_id(), hold.conversation_id(), &other)
            .await,
        Err(ArtifactReadError::Missing)
    );
    // Arm a read-only probe: no refusal below may reach physical bytes.
    *store.source.after_open.lock().unwrap() =
        Some(Box::new(|| panic!("retired request opened bytes")));
    store
        .release(
            hold.organization_id(),
            hold.conversation_id(),
            &release_evidence(),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .chunk(hold.organization_id(), hold.conversation_id(), &request)
            .await,
        Err(ArtifactReadError::Deleted)
    );
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), request.id())
            .await,
        Ok(Some(ArtifactState::Deleted))
    );
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn missing_or_truncated_blob_is_unavailable_and_same_length_corruption_is_unverified() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let hold = hold_for("org", CONVERSATION, b"bytes", b"bytes");
    let claim = claim(&store, &hold, b"bytes").await;
    store.confirm(&hold, &claim).await.unwrap();
    let request = range(&hold, &claim, 0, MAX_CHUNK_BYTES);
    let blob = store.files.root.join(blob_path(hold.stored().digest()));
    fs::write(&blob, b"wrong").unwrap();
    assert_eq!(
        store
            .chunk(hold.organization_id(), hold.conversation_id(), &request)
            .await
            .unwrap()
            .as_bytes(),
        b"wrong"
    );
    // The source returns only unverified bytes: the complete digest still disagrees.
    assert_ne!(digest_of(b"wrong"), hold.stored().digest());
    fs::write(&blob, b"x").unwrap();
    assert_eq!(
        store
            .chunk(hold.organization_id(), hold.conversation_id(), &request)
            .await,
        Err(ArtifactReadError::Unavailable)
    );
    fs::remove_file(&blob).unwrap();
    assert_eq!(
        store
            .chunk(hold.organization_id(), hold.conversation_id(), &request)
            .await,
        Err(ArtifactReadError::Unavailable)
    );
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), request.id())
            .await,
        Err(ArtifactReadError::Unavailable)
    );
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancelled_open_read_keeps_shared_capacity_and_actual_drain_while_release_retires() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(open(root.path()));
    let hold = hold_for("org", CONVERSATION, b"bytes", b"bytes");
    let claim = claim(&store, &hold, b"bytes").await;
    store.confirm(&hold, &claim).await.unwrap();
    let request = range(&hold, &claim, 0, 5);
    let (started, entered) = tokio::sync::oneshot::channel();
    let (release, resume) = mpsc::channel();
    *store.source.after_open.lock().unwrap() = Some(Box::new(move || {
        let _ = started.send(());
        resume.recv().unwrap();
    }));
    let waiter = {
        let (store, hold, request) = (store.clone(), hold.clone(), request.clone());
        tokio::spawn(async move {
            store
                .chunk(hold.organization_id(), hold.conversation_id(), &request)
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .unwrap()
        .unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), request.id())
            .await,
        Err(ArtifactReadError::Busy)
    );
    assert_eq!(
        store
            .chunk(hold.organization_id(), hold.conversation_id(), &request)
            .await,
        Err(ArtifactReadError::Busy)
    );
    // Opening was ordered before retirement. The actual file remains owned by
    // the held worker while the active registration becomes a tombstone.
    let retired = store
        .release(
            hold.organization_id(),
            hold.conversation_id(),
            &release_evidence(),
        )
        .await
        .unwrap();
    assert_eq!(retired.released.len(), 1);
    assert_eq!(retired.failures, 0);
    assert!(
        tokio::time::timeout(Duration::from_millis(30), store.shutdown())
            .await
            .is_err()
    );
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), request.id())
            .await,
        Err(ArtifactReadError::Unavailable)
    );
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), store.shutdown())
        .await
        .unwrap()
        .unwrap();
    store.shutdown().await.unwrap();
    let reopened = open(root.path());
    assert_eq!(
        reopened
            .manifest(hold.organization_id(), hold.conversation_id(), request.id())
            .await,
        Ok(Some(ArtifactState::Deleted))
    );
    assert_eq!(
        reopened
            .chunk(hold.organization_id(), hold.conversation_id(), &request)
            .await,
        Err(ArtifactReadError::Deleted)
    );
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn physical_source_panic_is_retained_by_the_shared_worker_and_drain() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let hold = hold_for("org", CONVERSATION, b"bytes", b"bytes");
    let claim = claim(&store, &hold, b"bytes").await;
    store.confirm(&hold, &claim).await.unwrap();
    *store.source.after_open.lock().unwrap() = Some(Box::new(|| panic!("source read panic probe")));
    let request = range(&hold, &claim, 0, 1);
    assert_eq!(
        store
            .chunk(hold.organization_id(), hold.conversation_id(), &request)
            .await,
        Err(ArtifactReadError::WorkerPanicked)
    );
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), request.id())
            .await,
        Err(ArtifactReadError::WorkerPanicked)
    );
    assert_eq!(
        store.shutdown().await,
        Err(ArtifactReadError::WorkerPanicked)
    );
    assert_eq!(
        store.shutdown().await,
        Err(ArtifactReadError::WorkerPanicked)
    );
}

#[tokio::test]
async fn an_admitted_open_range_finishes_after_retirement_but_later_reads_are_deleted() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(open(root.path()));
    let hold = hold_for("org", CONVERSATION, b"bytes", b"bytes");
    let claim = claim(&store, &hold, b"bytes").await;
    store.confirm(&hold, &claim).await.unwrap();
    let request = range(&hold, &claim, 1, 3);
    let (started, entered) = tokio::sync::oneshot::channel();
    let (release, resume) = mpsc::channel();
    *store.source.after_open.lock().unwrap() = Some(Box::new(move || {
        let _ = started.send(());
        resume.recv().unwrap();
    }));
    let waiter = {
        let (store, hold, request) = (store.clone(), hold.clone(), request.clone());
        tokio::spawn(async move {
            store
                .chunk(hold.organization_id(), hold.conversation_id(), &request)
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .unwrap()
        .unwrap();
    let retired = store
        .release(
            hold.organization_id(),
            hold.conversation_id(),
            &release_evidence(),
        )
        .await
        .unwrap();
    assert_eq!(retired.released.len(), 1);
    assert_eq!(retired.failures, 0);
    release.send(()).unwrap();
    assert_eq!(waiter.await.unwrap().unwrap().as_bytes(), b"yte");
    assert_eq!(
        store
            .chunk(hold.organization_id(), hold.conversation_id(), &request)
            .await,
        Err(ArtifactReadError::Deleted)
    );
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), request.id())
            .await,
        Ok(Some(ArtifactState::Deleted))
    );
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_physically_blocked_manifest_owns_the_same_nonwaiting_source_slot() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(open(root.path()));
    let hold = hold_for("org", CONVERSATION, b"bytes", b"bytes");
    let claim = claim(&store, &hold, b"bytes").await;
    store.confirm(&hold, &claim).await.unwrap();
    let request = range(&hold, &claim, 0, 1);
    let (started, entered) = tokio::sync::oneshot::channel();
    let (release, resume) = mpsc::channel();
    let locked = {
        let files = store.files.clone();
        std::thread::spawn(move || {
            let _changes = files.lock().unwrap();
            let _ = started.send(());
            // A failed assertion also releases the physical lock when its sender drops.
            let _ = resume.recv();
        })
    };
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .unwrap()
        .unwrap();
    let mut waiter = store.manifest(hold.organization_id(), hold.conversation_id(), request.id());
    // The first poll admits the real physical read before awaiting its answer.
    // Dropping that pending caller must leave the source slot charged to the worker.
    poll_fn(|cx| {
        assert!(waiter.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(waiter);
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), request.id())
            .await,
        Err(ArtifactReadError::Busy)
    );
    assert_eq!(
        store
            .chunk(hold.organization_id(), hold.conversation_id(), &request)
            .await,
        Err(ArtifactReadError::Busy)
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(30), store.shutdown())
            .await
            .is_err()
    );
    release.send(()).unwrap();
    locked.join().unwrap();
    tokio::time::timeout(Duration::from_secs(5), store.shutdown())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn a_real_record_directory_disappearance_before_sync_cannot_publish_previous_facts() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let hold = hold_for("org", CONVERSATION, b"bytes", b"bytes");
    let claim = claim(&store, &hold, b"bytes").await;
    store.confirm(&hold, &claim).await.unwrap();
    let request = range(&hold, &claim, 0, 1);
    let current = store.files.root.join(hold_directory(
        hold.organization_id(),
        hold.conversation_id(),
    ));
    let held = current.with_file_name("relocated-for-source-probe");
    for manifest in [true, false] {
        let (probe_current, probe_held) = (current.clone(), held.clone());
        *store.files.before_source_sync.lock().unwrap() = Some(Box::new(move || {
            fs::rename(probe_current, probe_held).unwrap();
        }));
        if manifest {
            assert_eq!(
                store
                    .manifest(hold.organization_id(), hold.conversation_id(), request.id())
                    .await,
                Err(ArtifactReadError::Unavailable)
            );
        } else {
            assert_eq!(
                store
                    .chunk(hold.organization_id(), hold.conversation_id(), &request)
                    .await,
                Err(ArtifactReadError::Unavailable)
            );
        }
        fs::rename(&held, &current).unwrap();
    }
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), request.id())
            .await,
        Ok(Some(ArtifactState::Live(hold.stored().clone())))
    );
    store.shutdown().await.unwrap();
}
