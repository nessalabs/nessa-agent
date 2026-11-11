//! Prepared physical observations cannot overwrite a newer native checkpoint.
use super::*;

#[tokio::test]
async fn stale_head_and_page_from_an_absent_cursor_do_not_refreeze_a_new_source() {
    let f = Fixture::new().await;
    let checkpoint = f
        .store
        .source_checkpoint(f.registration.id, f.conversation.clone())
        .await
        .unwrap();
    assert!(checkpoint.is_none());
    let old = f.record(1);
    let observed = old.scope.clone();
    let replacement = replaced_source(f.record(1));
    let current_scope = replacement.scope.clone();
    f.store.queue(replacement).await.unwrap();
    let files = f.audits();
    assert_eq!(
        f.store
            .source_after(
                f.registration.id,
                f.conversation.clone(),
                checkpoint,
                observed,
                QueueInitiator::Live,
                2000
            )
            .await,
        Err(OutboxError::Conflict)
    );
    let mut stale = old;
    stale.expected_source = None;
    assert_eq!(f.store.queue(stale).await, Err(OutboxError::Conflict));
    let saved = f
        .store
        .source_checkpoint(f.registration.id, f.conversation.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.scope, current_scope);
    assert!(!saved.reset_required);
    assert_eq!(saved.position, 1);
    assert_eq!(f.audits(), files);
}

#[tokio::test]
async fn checkpoint_advance_during_head_read_refuses_instead_of_returning_a_newer_position() {
    let f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    let checkpoint = f
        .store
        .source_checkpoint(f.registration.id, f.conversation.clone())
        .await
        .unwrap();
    let observed = checkpoint.as_ref().unwrap().scope.clone();
    f.store.queue(f.record(2)).await.unwrap();
    let files = f.audits();
    assert_eq!(
        f.store
            .source_after(
                f.registration.id,
                f.conversation.clone(),
                checkpoint,
                observed,
                QueueInitiator::Live,
                2000
            )
            .await,
        Err(OutboxError::Conflict)
    );
    assert_eq!(f.audits(), files);
}

#[tokio::test]
async fn committed_freeze_rejects_prepared_head_queue_and_admission_without_new_evidence() {
    let f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    let pending = f.store.due(2000).await.unwrap().pop().unwrap();
    let checkpoint = f
        .store
        .source_checkpoint(f.registration.id, f.conversation.clone())
        .await
        .unwrap();
    let original_scope = checkpoint.as_ref().unwrap().scope.clone();
    let replacement = replaced_source(f.record(1));
    assert_eq!(
        f.store.queue(replacement).await,
        Err(OutboxError::ResetRequired)
    );
    let files = f.audits();
    assert_eq!(
        f.store
            .source_after(
                f.registration.id,
                f.conversation.clone(),
                checkpoint,
                original_scope,
                QueueInitiator::Live,
                2000
            )
            .await,
        Err(OutboxError::Conflict)
    );
    assert_eq!(
        f.store.queue(f.record(2)).await,
        Err(OutboxError::ResetRequired)
    );
    let attempt =
        f.store
            .delivery_attempt(f.registration.id, pending.delivery.clone(), pending.capture);
    let sending = pending.delivery.admit(7, 2000).unwrap();
    assert_eq!(
        f.store
            .admit_before(
                &attempt,
                &sending,
                2000,
                Instant::now() + std::time::Duration::from_secs(15)
            )
            .await,
        Err(OutboxError::Conflict)
    );
    assert!(!f.store.attempt_pending());
    assert_eq!(f.audits(), files);
}
