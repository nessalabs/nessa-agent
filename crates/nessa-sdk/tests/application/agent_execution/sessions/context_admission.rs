//! Admission boundaries are checked on custom snapshots and semantic publication.
use super::{terminal_prefix, validate};
use crate::application::agent_execution::{
    providers::ProviderIdentity,
    sessions::{
        records::continuation::Continuation,
        test_support::{snapshot, text, turn},
        ProviderContext, SessionChange, StorageError,
    },
};
use crate::domain::agent_execution::sessions::ExecutionSessionId;

fn context(id: &str, watermark: usize) -> ProviderContext {
    ProviderContext::recorded_after(ExecutionSessionId::new(id).unwrap(), watermark)
}

#[test]
fn custom_snapshot_refuses_out_of_range_and_pending_prefix_watermarks() {
    let mut pending = turn("pending", text(), Vec::new());
    pending.events.clear();
    pending.result = None;
    pending.local_outcome = None;
    let mut saved = snapshot(vec![turn("completed", text(), Vec::new()), pending]);
    assert_eq!(terminal_prefix(&saved.invocations), 1);
    for watermark in [2, 3] {
        saved.provider_context = context("new-context", watermark);
        assert!(matches!(validate(&saved), Err(StorageError::Corrupt(_))));
        assert!(matches!(
            Continuation::restore(Some(saved.clone())),
            Err(StorageError::Corrupt(_))
        ));
    }
    saved.provider_context = context("new-context", 1);
    assert_eq!(validate(&saved), Ok(()));
    assert!(saved
        .provider_context
        .has_admitted_work(saved.invocations.len()));
}

#[test]
fn fresh_context_publication_preserves_an_earlier_captured_terminal_prefix() {
    use crate::domain::agent_execution::sessions::ProviderContextAdmissionError;
    // Opening began after `first`. Later work became terminal while it waited.
    let captured = context("new-context", 1);
    let saved = snapshot(vec![
        turn("first", text(), Vec::new()),
        turn("second", text(), Vec::new()),
    ]);
    assert_eq!(terminal_prefix(&saved.invocations), 2);
    let mut continuation = Continuation::restore(Some(saved.clone())).unwrap();
    continuation
        .apply_unit(&[SessionChange::ProviderContext {
            before: saved.provider_context.clone(),
            after: captured.clone(),
        }])
        .unwrap();
    let published = continuation.snapshot.unwrap();
    assert_eq!(published.provider_context, captured);
    assert!(published.provider_context.has_admitted_work(2));
    for watermark in 0..=2 {
        let mut continuation = Continuation::restore(Some(saved.clone())).unwrap();
        continuation
            .apply_unit(&[SessionChange::ProviderContext {
                before: saved.provider_context.clone(),
                after: context("new-context", watermark),
            }])
            .unwrap();
    }
    for watermark in [3, usize::MAX] {
        let mut continuation = Continuation::restore(Some(saved.clone())).unwrap();
        assert_eq!(
            continuation.apply_unit(&[SessionChange::ProviderContext {
                before: saved.provider_context.clone(),
                after: context("new-context", watermark),
            }]),
            Err(StorageError::Corrupt(
                ProviderContextAdmissionError::InvalidWatermark.to_string(),
            )),
        );
        assert_eq!(continuation.snapshot.unwrap(), saved);
    }
}

#[test]
fn repeated_native_id_cannot_erase_admission_without_prior_namespace_change() {
    let saved = snapshot(vec![turn("first", text(), Vec::new())]);
    let next = context("provider", 1);
    let revision = SessionChange::ProviderContext {
        before: saved.provider_context.clone(),
        after: next.clone(),
    };
    let mut continuation = Continuation::restore(Some(saved.clone())).unwrap();
    assert!(matches!(
        continuation.apply_unit(std::slice::from_ref(&revision)),
        Err(StorageError::Corrupt(_))
    ));
    assert_eq!(continuation.snapshot.as_ref(), Some(&saved));
    continuation
        .apply_unit(&[
            SessionChange::ProviderIdentityChanged {
                before: saved.provider.clone(),
                after: ProviderIdentity::new("provider", "model", "another-workspace").unwrap(),
            },
            revision,
        ])
        .unwrap();
    let published = continuation.snapshot.unwrap();
    assert_eq!(published.provider_context, next);
    assert!(!published.provider_context.has_admitted_work(1));
}

#[test]
fn namespace_evidence_following_reset_does_not_authorize_it() {
    let saved = snapshot(vec![turn("first", text(), Vec::new())]);
    let mut continuation = Continuation::restore(Some(saved.clone())).unwrap();
    assert!(matches!(
        continuation.apply_unit(&[
            SessionChange::ProviderContext {
                before: saved.provider_context.clone(),
                after: context("provider", 1),
            },
            SessionChange::ProviderIdentityChanged {
                before: saved.provider.clone(),
                after: ProviderIdentity::new("provider", "model", "another-workspace").unwrap(),
            },
        ]),
        Err(StorageError::Corrupt(_))
    ));
    assert_eq!(continuation.snapshot.unwrap(), saved);
}

#[test]
fn opened_context_cannot_claim_a_preceding_invocation() {
    let saved = snapshot(Vec::new());
    let mut continuation = Continuation::empty();
    assert!(matches!(
        continuation.apply_unit(&[SessionChange::Opened {
            id: saved.id,
            provider: saved.provider,
            context: context("new-context", 1),
        }]),
        Err(StorageError::Corrupt(_))
    ));
    assert!(continuation.snapshot.is_none());
}

#[test]
fn a_result_saved_before_final_scheduling_does_not_hide_pending_work() {
    use crate::application::agent_execution::sessions::InvocationSchedulingEvent;
    use crate::domain::agent_execution::executions::{
        InvocationKind, InvocationStage, SchedulingCause,
    };
    let mut settling = turn("settling", text(), Vec::new());
    settling.scheduling.push(InvocationSchedulingEvent {
        kind: InvocationKind::Queued,
        target: None,
        before: None,
        stage: InvocationStage::Running,
        cause: SchedulingCause::Dispatched,
        actor: None,
    });
    assert!(settling.result.is_some());
    assert_eq!(terminal_prefix(&[settling]), 0);
}

#[test]
fn namespace_round_trip_cannot_reset_the_original_context() {
    let saved = snapshot(vec![turn("first", text(), Vec::new())]);
    let other = ProviderIdentity::new("provider", "model", "another-workspace").unwrap();
    let mut continuation = Continuation::restore(Some(saved.clone())).unwrap();
    assert!(matches!(
        continuation.apply_unit(&[
            SessionChange::ProviderIdentityChanged {
                before: saved.provider.clone(),
                after: other.clone(),
            },
            SessionChange::ProviderIdentityChanged {
                before: other,
                after: saved.provider.clone(),
            },
            SessionChange::ProviderContext {
                before: saved.provider_context.clone(),
                after: context("provider", 1),
            },
        ]),
        Err(StorageError::Corrupt(_))
    ));
    assert_eq!(continuation.snapshot.unwrap(), saved);
}

#[test]
fn queued_admission_prevents_context_clear_without_provider_observations() {
    use crate::application::agent_execution::sessions::InvocationSchedulingEvent;
    use crate::domain::agent_execution::executions::{
        InvocationKind, InvocationStage, SchedulingCause, SubmissionMode,
    };
    let mut queued = turn("queued", text(), Vec::new());
    queued.events.clear();
    queued.result = None;
    queued.local_outcome = None;
    queued.submission = SubmissionMode::Queued;
    queued.scheduling.push(InvocationSchedulingEvent {
        kind: InvocationKind::Queued,
        target: None,
        before: None,
        stage: InvocationStage::Queued,
        cause: SchedulingCause::Submitted,
        actor: Some(queued.actor.clone()),
    });
    let saved = snapshot(vec![queued]);
    assert_eq!(validate(&saved), Ok(()));
    let mut continuation = Continuation::restore(Some(saved.clone())).unwrap();
    assert!(matches!(
        continuation.apply_unit(&[SessionChange::ProviderContext {
            before: saved.provider_context.clone(),
            after: ProviderContext::Absent,
        }]),
        Err(StorageError::Corrupt(_))
    ));
    assert_eq!(continuation.snapshot.as_ref(), Some(&saved));
    let mut initial_pending = saved;
    initial_pending.provider_context = ProviderContext::Absent;
    assert_eq!(validate(&initial_pending), Ok(()));
}
