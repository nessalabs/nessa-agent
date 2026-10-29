//! Native evidence tests; public authorization and physical source are separate.
use super::*;
use crate::webhooks::infrastructure::outbox::{cursor, ResetCommand};
use std::time::Duration;
async fn frozen(f: &Fixture) -> cursor::Cursor {
    f.store.queue(f.record(1)).await.unwrap();
    assert_eq!(
        f.store.queue(replaced_source(f.record(2))).await,
        Err(OutboxError::ResetRequired)
    );
    f.store
        .source_checkpoint(f.registration.id, f.conversation.clone())
        .await
        .unwrap()
        .unwrap()
}
fn command(f: &Fixture, expected: cursor::Cursor) -> ResetCommand {
    ResetCommand {
        request: expected.reset.unwrap(),
        registration: f.registration.id,
        credential: f.registration.target.credential.clone(),
        issued_at_ms: 1000,
        actor: f.registration.target.owner.clone(),
        conversation: f.conversation.clone(),
        expected,
        observed: replaced_source(f.record(1)).scope,
        at_ms: 2000,
        authority_expires_at_ms: None,
    }
}
#[tokio::test]
async fn observed_choice_is_scoped_and_replay_survives_later_replacement_and_restart() {
    let mut f = Fixture::new().await;
    let before = frozen(&f).await;
    let request = before.reset.unwrap();
    let result = f
        .store
        .reset_source(command(&f, before.clone()))
        .await
        .unwrap();
    assert_eq!(result.request, request);
    assert_eq!(result.scope, replaced_source(f.record(1)).scope);
    let following = f
        .store
        .source_checkpoint(f.registration.id, f.conversation.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(following.position, 0);
    assert!(!following.reset_required);
    assert_eq!(following.reset, Some(result.receipt));
    // An old prepared A page cannot undo the reset to B.
    assert_eq!(f.store.queue(f.record(2)).await, Err(OutboxError::Conflict));
    let original_files = f.audits();
    let mut wrong = command(&f, before.clone());
    wrong.actor = PrincipalId::new("foreign-owner").unwrap();
    assert!(matches!(
        f.store.reset_source(wrong).await,
        Err(OutboxError::Conflict)
    ));
    assert_eq!(f.audits(), original_files);
    let mut changed = f.record(1);
    changed.expected_source = Some(following.scope.clone());
    changed.capture = following.reset;
    // Observing A after B requires a new explicit reset even though A existed before.
    assert_eq!(
        f.store.queue(changed).await,
        Err(OutboxError::ResetRequired)
    );
    let later = f
        .store
        .source_checkpoint(f.registration.id, f.conversation.clone())
        .await
        .unwrap()
        .unwrap();
    assert_ne!(later.reset, Some(request));
    let mut next = command(&f, later.clone());
    next.observed = f.record(1).scope;
    let second = f.store.reset_source(next).await.unwrap();
    assert_ne!(second.receipt, result.receipt);
    let reopened = reopen_store(&mut f).await.unwrap();
    let replay = reopened
        .reset_receipt(
            request,
            f.registration.id,
            f.registration.target.credential.clone(),
            1000,
            f.registration.target.owner.clone(),
            f.conversation.clone(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay.receipt, result.receipt);
    assert_eq!(replay.scope, result.scope);
    assert_eq!(replay.at_ms, 2000);
    let current = reopened
        .source_checkpoint(f.registration.id, f.conversation.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.reset, Some(second.receipt));
    assert_eq!(current.scope, second.scope);
}
#[tokio::test]
async fn failed_reset_publication_retains_choice_and_replay_does_not_reselect() {
    let f = Fixture::new().await;
    let before = frozen(&f).await;
    let request = before.reset.unwrap();
    let held = f.audit.with_extension("held");
    std::fs::rename(&f.audit, &held).unwrap();
    // Fail the new reset publication after the reset SQL commit, not before it.
    assert!(matches!(
        f.store.reset_source(command(&f, before.clone())).await,
        Err(OutboxError::Unavailable)
    ));
    let stored = f
        .store
        .reset_receipt(
            request,
            f.registration.id,
            f.registration.target.credential.clone(),
            1000,
            f.registration.target.owner.clone(),
            f.conversation.clone(),
        )
        .await
        .unwrap();
    let receipt =
        stored.expect("reset SQL must commit its immutable choice before failed publication");
    {
        let c = f.store.owner.connection.lock().unwrap();
        c.execute("UPDATE cursors SET position=1", []).unwrap();
    }
    assert!(
        matches!(
            f.store.reset_source(command(&f, before.clone())).await,
            Err(OutboxError::Invalid)
        ),
        "owed owner selection cannot claim its zero checkpoint after contradictory cursor progress"
    );
    {
        let c = f.store.owner.connection.lock().unwrap();
        c.execute("UPDATE cursors SET position=0", []).unwrap();
    }
    std::fs::rename(&held, &f.audit).unwrap();
    let mut replay = command(&f, before);
    replay.observed = f.record(1).scope;
    replay.at_ms = 9999;
    let result = f.store.reset_source(replay).await.unwrap();
    assert_eq!(result.receipt, receipt.receipt);
    assert_eq!(result.scope, receipt.scope);
    assert_eq!(result.at_ms, receipt.at_ms);
}

async fn freeze_at(f: &Fixture, scope: Scope) -> cursor::Cursor {
    let prior = f
        .store
        .source_checkpoint(f.registration.id, f.conversation.clone())
        .await
        .unwrap();
    assert_eq!(
        f.store
            .source_after(
                f.registration.id,
                f.conversation.clone(),
                prior,
                scope,
                QueueInitiator::Live,
                2000
            )
            .await,
        Err(OutboxError::ResetRequired)
    );
    f.store
        .source_checkpoint(f.registration.id, f.conversation.clone())
        .await
        .unwrap()
        .unwrap()
}
#[tokio::test]
async fn identical_looking_checkpoint_after_two_resets_rejects_old_prepared_generation() {
    let f = Fixture::new().await;
    let first = frozen(&f).await;
    f.store.reset_source(command(&f, first)).await.unwrap(); // B at zero.
    let a = f.record(1).scope;
    let b = replaced_source(f.record(1)).scope;
    let next = freeze_at(&f, a.clone()).await;
    let mut select_a = command(&f, next);
    select_a.observed = a.clone();
    f.store.reset_source(select_a).await.unwrap();
    let old = f
        .store
        .source_checkpoint(f.registration.id, f.conversation.clone())
        .await
        .unwrap()
        .unwrap();
    let frozen_a = freeze_at(&f, b.clone()).await;
    f.store.reset_source(command(&f, frozen_a)).await.unwrap();
    let frozen_b = freeze_at(&f, a.clone()).await;
    let mut select_a = command(&f, frozen_b);
    select_a.observed = a.clone();
    f.store.reset_source(select_a).await.unwrap();
    let current = f
        .store
        .source_checkpoint(f.registration.id, f.conversation.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.scope, old.scope);
    assert_eq!(current.position, old.position);
    assert_ne!(current.reset, old.reset);
    let files = f.audits();
    assert_eq!(
        f.store
            .source_after(
                f.registration.id,
                f.conversation.clone(),
                Some(old.clone()),
                a.clone(),
                QueueInitiator::Live,
                2000
            )
            .await,
        Err(OutboxError::Conflict)
    );
    let mut stale = f.record(1);
    stale.capture = old.reset;
    stale.expected_source = Some(a);
    assert_eq!(f.store.queue(stale).await, Err(OutboxError::Conflict));
    assert_eq!(f.audits(), files);
}
#[tokio::test]
async fn expiry_refuses_native_selection_and_replay_without_publication_effects() {
    let f = Fixture::new().await;
    let before = frozen(&f).await;
    let mut expired = command(&f, before.clone());
    expired.authority_expires_at_ms = Some(10000);
    let files = f.audits();
    assert!(matches!(
        f.store.reset_source(expired).await,
        Err(OutboxError::Conflict)
    ));
    assert_eq!(f.audits(), files);
    let result = f
        .store
        .reset_source(command(&f, before.clone()))
        .await
        .unwrap();
    let mut expired = command(&f, before);
    expired.authority_expires_at_ms = Some(10000);
    let files = f.audits();
    assert!(matches!(
        f.store.reset_source(expired).await,
        Err(OutboxError::Conflict)
    ));
    assert_eq!(f.audits(), files);
    assert_eq!(
        f.store
            .source_checkpoint(f.registration.id, f.conversation.clone())
            .await
            .unwrap()
            .unwrap()
            .reset,
        Some(result.receipt)
    );
}

#[tokio::test]
async fn failed_freeze_publication_prevents_reset_and_keeps_original_capture() {
    let f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    let before = f
        .store
        .source_checkpoint(f.registration.id, f.conversation.clone())
        .await
        .unwrap();
    let held = f.audit.with_extension("held");
    std::fs::rename(&f.audit, &held).unwrap();
    let observed = replaced_source(f.record(1)).scope;
    assert_eq!(
        f.store
            .source_after(
                f.registration.id,
                f.conversation.clone(),
                before,
                observed,
                QueueInitiator::Live,
                2000
            )
            .await,
        Err(OutboxError::Unavailable)
    );
    let frozen = f
        .store
        .source_checkpoint(f.registration.id, f.conversation.clone())
        .await
        .unwrap()
        .unwrap();
    assert!(frozen.reset_required);
    let request = frozen.reset;
    assert!(matches!(
        f.store.reset_source(command(&f, frozen)).await,
        Err(OutboxError::Unavailable)
    ));
    let current = f
        .store
        .source_checkpoint(f.registration.id, f.conversation.clone())
        .await
        .unwrap()
        .unwrap();
    assert!(current.reset_required);
    assert_eq!(current.reset, request);
    assert!(!f.store.attempt_pending());
    assert!(f
        .store
        .reset_receipt(
            request.unwrap(),
            f.registration.id,
            f.registration.target.credential.clone(),
            1000,
            f.registration.target.owner.clone(),
            f.conversation.clone()
        )
        .await
        .unwrap()
        .is_none());
    std::fs::rename(&held, &f.audit).unwrap();
}

#[tokio::test]
async fn dispatch_refuses_wrong_capture_even_with_exact_current_queued_snapshot() {
    let f = Fixture::new().await;
    let before = frozen(&f).await;
    let result = f.store.reset_source(command(&f, before)).await.unwrap();
    let mut record = replaced_source(f.record(1));
    record.expected_source = Some(result.scope);
    record.capture = Some(result.receipt);
    f.store.queue(record).await.unwrap();
    let pending = f
        .store
        .due(2000)
        .await
        .unwrap()
        .into_iter()
        .find(|d| d.delivery.phase() == Phase::Queued)
        .unwrap();
    let attempt = f
        .store
        .delivery_attempt(f.registration.id, pending.delivery.clone(), None);
    let sending = pending.delivery.admit(7, 2000).unwrap();
    let files = f.audits();
    assert_eq!(
        f.store
            .admit_before(
                &attempt,
                &sending,
                2000,
                Instant::now() + Duration::from_secs(15)
            )
            .await,
        Err(OutboxError::Conflict)
    );
    assert!(!f.store.attempt_pending());
    assert_eq!(f.audits(), files);
}

#[tokio::test]
async fn missing_or_substituted_reset_receipt_refuses_live_replay_and_cold_restore() {
    for missing in [false, true] {
        let mut f = Fixture::new().await;
        let before = frozen(&f).await;
        let result = f
            .store
            .reset_source(command(&f, before.clone()))
            .await
            .unwrap();
        {
            let c = f.store.owner.connection.lock().unwrap();
            if missing {
                c.execute(
                    "DELETE FROM audit WHERE id=?1",
                    [result.receipt.to_string()],
                )
                .unwrap();
            } else {
                c.execute("UPDATE audit SET record=json_set(record,'$.initiator','foreign-owner','$.selection.actor','foreign-owner') WHERE id=?1",[result.receipt.to_string()]).unwrap();
            }
        }
        let files = f.audits();
        assert!(
            matches!(
                f.store
                    .source_checkpoint(f.registration.id, f.conversation.clone())
                    .await,
                Err(OutboxError::Invalid)
            ),
            "a current cursor cannot borrow an erased or contradictory receipt"
        );
        assert!(matches!(
            f.store
                .reset_receipt(
                    result.request,
                    f.registration.id,
                    f.registration.target.credential.clone(),
                    1000,
                    f.registration.target.owner.clone(),
                    f.conversation.clone()
                )
                .await,
            Err(OutboxError::Invalid)
        ));
        assert!(matches!(
            f.store.reset_source(command(&f, before)).await,
            Err(OutboxError::Invalid)
        ));
        assert_eq!(
            f.audits(),
            files,
            "contradictory or absent receipt refuses before file effects"
        );
        assert!(matches!(
            reopen_store(&mut f).await,
            Err(OutboxError::Invalid)
        ));
    }
}
