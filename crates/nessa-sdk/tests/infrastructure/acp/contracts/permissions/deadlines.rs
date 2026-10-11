//! An unanswered review settles when its deadline passes, and the turn goes on.
use super::super::support::*;
use std::task::{Context, Waker};

const DENY_REASON: &str =
    "No human answered in time. Continue without this tool or try another approach.";
const ALLOW_REASON: &str =
    "No human answered in time. This read-only tool was allowed so the turn could continue.";

struct PendingReview {
    root: TempDir,
    opened: OpenedProviderSession,
    active: tokio::task::JoinHandle<Result<ExecutionOutcome, AgentError>>,
    clock: Arc<ManualClock>,
    audit: Arc<RecordingAudit>,
    id: PermissionId,
    deadline: PermissionReviewDeadline,
}

async fn pending(mode: &str) -> PendingReview {
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding, clock) = test_acp_binding_on_clock(mode, audit.clone(), None);
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "write").await;
    assert!(matches!(next(&mut opened).await, ExecutionUpdate::Tool(_)));
    let ExecutionUpdate::PermissionRequested { id, deadline, .. } = next(&mut opened).await else {
        panic!("expected permission");
    };
    let deadline = deadline.expect("a pending review names the deadline that will answer it");
    PendingReview {
        root,
        opened,
        active,
        clock,
        audit,
        id,
        deadline,
    }
}

async fn until_finished(opened: &mut OpenedProviderSession) -> Vec<ExecutionUpdate> {
    let mut updates = Vec::new();
    loop {
        let update = next(opened).await;
        let finished = matches!(update, ExecutionUpdate::Finished(_));
        updates.push(update);
        if finished {
            return updates;
        }
    }
}

fn answered(audit: &RecordingAudit) -> Vec<PermissionAnswerRecord> {
    audit.answers.lock().unwrap().clone()
}

fn assert_deadline_decided(
    records: &[PermissionAnswerRecord],
    class: PermissionToolClass,
    wait: Duration,
    effect: PermissionEffect,
    reason: &str,
    option: &str,
) {
    assert_eq!(records.len(), 2, "selected, then written: {records:?}");
    assert_eq!(records[0].delivery(), &PermissionAnswerDelivery::Selected);
    assert_eq!(records[1].delivery(), &PermissionAnswerDelivery::Written);
    for record in records {
        let basis = record.resolution().attribution().basis();
        let ApprovalBasis::Deadline(deadline) = basis else {
            panic!("the deadline is the decider, not {basis:?}");
        };
        assert_eq!(deadline.class(), class);
        assert_eq!(deadline.wait(), wait);
        assert_eq!(deadline.effect(), effect);
        assert_eq!(deadline.reason(), reason);
        let actor = record.resolution().attribution().actor();
        assert_eq!(actor.principal_id(), "permission-deadline");
        assert_eq!(actor.surface_id(), class.as_str());
        match record.resolution().request().state() {
            PermissionStateView::Answered {
                option_id,
                decision,
            } => {
                assert_eq!(option_id.as_str(), option);
                assert_eq!(decision.effect(), effect);
            }
            other => panic!("the deadline selects an option, got {other:?}"),
        }
    }
}

fn human_allow(id: PermissionId) -> PermissionAnswer {
    PermissionAnswer {
        attribution: ApprovalAttribution::new(
            ActionContext::new("reviewer", "nessa.panel", "human-answer").unwrap(),
            ApprovalBasis::Explicit,
        ),
        execution_id: ExecutionId::new("write").unwrap(),
        id,
        option_id: PermissionOptionId::new("approve-one").unwrap(),
    }
}

async fn close(clock: &ManualClock, opened: &OpenedProviderSession) {
    let closing = opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()));
    promptly(clock.passing(a_grace, closing))
        .await
        .into_result()
        .unwrap();
}

#[tokio::test]
async fn an_unanswered_review_is_denied_at_its_deadline_and_the_turn_continues() {
    let _slot = process_test_slot().await;
    let mut review = pending("permission-deadline").await;
    assert_eq!(review.deadline.class(), PermissionToolClass::Other);
    assert_eq!(review.deadline.wait(), Duration::from_secs(30));
    assert_eq!(review.deadline.effect(), PermissionEffect::Deny);
    assert_eq!(review.deadline.reason(), DENY_REASON);

    review
        .clock
        .run_out(|wait| wait.limit() == Duration::from_secs(30))
        .await;
    let updates = until_finished(&mut review.opened).await;
    assert_eq!(
        updates,
        vec![
            ExecutionUpdate::DeadlineAnswered {
                id: review.id.clone(),
                effect: PermissionEffect::Deny,
                reason: DENY_REASON.to_owned(),
            },
            ExecutionUpdate::Message(MessageChunk::text(DENY_REASON)),
            ExecutionUpdate::Finished(ExecutionOutcome::Completed),
        ]
    );
    assert_eq!(
        promptly(review.active).await.unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    assert_deadline_decided(
        &answered(&review.audit),
        PermissionToolClass::Other,
        Duration::from_secs(30),
        PermissionEffect::Deny,
        DENY_REASON,
        "deny-one",
    );
    let wire: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(review.root.path().join("permission-outcome")).unwrap(),
    )
    .unwrap();
    assert_eq!(wire["outcome"]["optionId"], "deny-one");
    assert_eq!(wire["_meta"]["deadlineReason"], DENY_REASON);
    close(&review.clock, &review.opened).await;
    assert_gone(&review.root, "pid");
}

#[tokio::test]
async fn an_unanswered_read_is_allowed_when_its_shorter_deadline_passes() {
    let _slot = process_test_slot().await;
    let mut review = pending("permission-read-deadline").await;
    assert_eq!(review.deadline.class(), PermissionToolClass::ReadOnly);
    assert_eq!(review.deadline.wait(), Duration::from_secs(8));
    assert_eq!(review.deadline.effect(), PermissionEffect::Allow);
    assert_eq!(review.deadline.reason(), ALLOW_REASON);

    review
        .clock
        .run_out(|wait| wait.limit() == Duration::from_secs(8))
        .await;
    let updates = until_finished(&mut review.opened).await;
    assert_eq!(
        updates,
        vec![
            ExecutionUpdate::DeadlineAnswered {
                id: review.id.clone(),
                effect: PermissionEffect::Allow,
                reason: ALLOW_REASON.to_owned(),
            },
            ExecutionUpdate::Message(MessageChunk::text(ALLOW_REASON)),
            ExecutionUpdate::Finished(ExecutionOutcome::Completed),
        ]
    );
    assert_eq!(
        promptly(review.active).await.unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    assert_deadline_decided(
        &answered(&review.audit),
        PermissionToolClass::ReadOnly,
        Duration::from_secs(8),
        PermissionEffect::Allow,
        ALLOW_REASON,
        "approve-one",
    );
    close(&review.clock, &review.opened).await;
    assert_gone(&review.root, "pid");
}

#[tokio::test]
async fn a_human_answer_before_the_deadline_is_the_one_that_stands() {
    let _slot = process_test_slot().await;
    let mut review = pending("permission").await;
    let resolution = review
        .opened
        .session
        .answer_permission(human_allow(review.id.clone()))
        .await
        .unwrap();
    assert!(matches!(
        resolution.attribution().basis(),
        ApprovalBasis::Explicit
    ));
    let updates = until_finished(&mut review.opened).await;
    assert!(
        updates.iter().any(|update| matches!(
            update,
            ExecutionUpdate::Finished(ExecutionOutcome::Completed)
        )),
        "{updates:?}"
    );
    assert!(updates
        .iter()
        .all(|update| !matches!(update, ExecutionUpdate::DeadlineAnswered { .. })));
    assert_eq!(
        promptly(review.active).await.unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    assert!(review.root.path().join("fixture.txt").exists());
    let records = answered(&review.audit);
    assert!(records.iter().all(|record| matches!(
        record.resolution().attribution().basis(),
        ApprovalBasis::Explicit
    )));
    assert_eq!(
        records[0].resolution().attribution().actor().principal_id(),
        "reviewer"
    );

    // The deadline later passing must not replace the answer already given.
    review.clock.advance(Duration::from_secs(30));
    let late = timeout(Duration::from_millis(400), review.opened.events.next()).await;
    assert!(
        late.is_err(),
        "a deadline that passes after a person answered must stay quiet"
    );
    assert!(answered(&review.audit).iter().all(|record| matches!(
        record.resolution().attribution().basis(),
        ApprovalBasis::Explicit
    )));
    let choice: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(review.root.path().join("permission-outcome")).unwrap(),
    )
    .unwrap();
    assert_eq!(choice["optionId"], "approve-one");
    close(&review.clock, &review.opened).await;
    assert_gone(&review.root, "pid");
}

#[tokio::test(flavor = "current_thread")]
async fn an_answer_already_waiting_at_the_deadline_beats_the_deadline() {
    let _slot = process_test_slot().await;
    let mut review = pending("permission").await;
    timeout(Duration::from_secs(5), async {
        loop {
            if review
                .clock
                .waits()
                .iter()
                .any(|wait| wait.limit() == Duration::from_secs(30))
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the review's deadline was never armed");

    // Admit the person's answer without letting the worker run, then reach
    // the deadline. Both are ready in the same wait; the answer was already
    // queued, so it is the one taken.
    let mut answering = review
        .opened
        .session
        .answer_permission(human_allow(review.id.clone()));
    let mut context = Context::from_waker(Waker::noop());
    assert!(answering.as_mut().poll(&mut context).is_pending());
    review.clock.advance(Duration::from_secs(30));

    let resolution = timeout(Duration::from_secs(5), answering)
        .await
        .expect("the admitted answer was not taken")
        .unwrap();
    assert!(matches!(
        resolution.attribution().basis(),
        ApprovalBasis::Explicit
    ));
    assert_eq!(resolution.attribution().actor().principal_id(), "reviewer");
    let updates = until_finished(&mut review.opened).await;
    assert!(updates
        .iter()
        .all(|update| !matches!(update, ExecutionUpdate::DeadlineAnswered { .. })));
    assert_eq!(
        promptly(review.active).await.unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    assert!(review.root.path().join("fixture.txt").exists());
    assert!(answered(&review.audit).iter().all(|record| matches!(
        record.resolution().attribution().basis(),
        ApprovalBasis::Explicit
    )));
    close(&review.clock, &review.opened).await;
    assert_gone(&review.root, "pid");
}

#[tokio::test]
async fn a_configured_read_deadline_replaces_the_standard_allow() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, mut config, model) = test_acp_configuration("permission-read-deadline", 16);
    config.execution_timeout = None;
    config.permission_deadlines = PermissionDeadlinePolicy::new(
        PermissionReviewDeadline::new(
            PermissionToolClass::ReadOnly,
            Duration::from_secs(1),
            PermissionEffect::Deny,
            DENY_REASON,
        )
        .unwrap(),
        PermissionDeadlinePolicy::standard()
            .for_class(PermissionToolClass::Other)
            .clone(),
    )
    .unwrap();
    let clock = manual_clock(&mut config);
    let binding = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        audit.clone(),
    )
    .unwrap();
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "write").await;
    assert!(matches!(next(&mut opened).await, ExecutionUpdate::Tool(_)));
    let ExecutionUpdate::PermissionRequested { deadline, .. } = next(&mut opened).await else {
        panic!("expected permission");
    };
    let deadline = deadline.unwrap();
    assert_eq!(deadline.wait(), Duration::from_secs(1));
    assert_eq!(deadline.effect(), PermissionEffect::Deny);
    clock
        .run_out(|wait| wait.limit() == Duration::from_secs(1))
        .await;
    let updates = until_finished(&mut opened).await;
    assert!(matches!(
        updates.first(),
        Some(ExecutionUpdate::DeadlineAnswered {
            effect: PermissionEffect::Deny,
            ..
        })
    ));
    assert!(updates.iter().any(|update| matches!(
        update,
        ExecutionUpdate::Message(chunk) if chunk.as_str() == DENY_REASON
    )));
    assert_eq!(
        promptly(active).await.unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    assert_deadline_decided(
        &answered(&audit),
        PermissionToolClass::ReadOnly,
        Duration::from_secs(1),
        PermissionEffect::Deny,
        DENY_REASON,
        "deny-one",
    );
    close(&clock, &opened).await;
    assert_gone(&root, "pid");
}

/// Answer records fail. Everything else is kept. The deadline must still
/// answer the provider: a person is not there to retry the audit.
struct AnswerAuditThatFails;
impl ExecutionAudit for AnswerAuditThatFails {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            if matches!(record, ExecutionAuditRecord::Answered(_)) {
                Err(AgentError::AuditFailure)
            } else {
                Ok(())
            }
        })
    }
}

#[tokio::test]
async fn a_failed_answer_audit_still_denies_and_the_turn_continues() {
    let _slot = process_test_slot().await;
    let (root, mut config, model) = test_acp_configuration("permission-deadline", 16);
    config.execution_timeout = None;
    let clock = manual_clock(&mut config);
    let binding = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(AnswerAuditThatFails),
    )
    .unwrap();
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "write").await;
    assert!(matches!(next(&mut opened).await, ExecutionUpdate::Tool(_)));
    assert!(matches!(
        next(&mut opened).await,
        ExecutionUpdate::PermissionRequested { .. }
    ));
    clock
        .run_out(|wait| wait.limit() == Duration::from_secs(30))
        .await;
    let updates = until_finished(&mut opened).await;
    assert!(updates.iter().any(|update| matches!(
        update,
        ExecutionUpdate::Message(chunk) if chunk.as_str() == DENY_REASON
    )));
    assert!(matches!(
        updates.last(),
        Some(ExecutionUpdate::Finished(ExecutionOutcome::Completed))
    ));
    assert_eq!(
        promptly(active).await.unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    // The two dropped answer records are retained, and shutdown says so.
    let closing = opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()));
    let closed = promptly(clock.passing(a_grace, closing))
        .await
        .into_result();
    assert!(
        matches!(closed, Err(AgentError::MultipleOperationFailures { .. })),
        "{closed:?}"
    );
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn a_review_with_no_deny_option_is_cancelled_and_the_turn_continues() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding, clock) =
        test_acp_binding_on_clock("permission-deadline-allow-only", audit.clone(), None);
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "write").await;
    assert!(matches!(next(&mut opened).await, ExecutionUpdate::Tool(_)));
    let ExecutionUpdate::PermissionRequested { id, deadline, .. } = next(&mut opened).await else {
        panic!("expected permission");
    };
    let deadline = deadline.unwrap();
    assert_eq!(deadline.effect(), PermissionEffect::Deny);
    assert!(deadline.reason().contains("Continue without this tool"));
    clock
        .run_out(|wait| wait.limit() == Duration::from_secs(30))
        .await;
    let updates = until_finished(&mut opened).await;
    assert!(matches!(
        updates.first(),
        Some(ExecutionUpdate::PermissionCancelled(cancellation))
            if cancellation.request().id() == &id
                && cancellation.origin() == &CancellationOrigin::Runtime
    ));
    assert!(updates.iter().any(|update| matches!(
        update,
        ExecutionUpdate::DeadlineAnswered {
            effect: PermissionEffect::Deny,
            reason,
            ..
        } if reason == deadline.reason()
    )));
    assert!(updates.iter().any(|update| matches!(
        update,
        ExecutionUpdate::Message(chunk) if chunk.as_str() == "cancelled"
    )));
    assert_eq!(
        promptly(active).await.unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    assert!(answered(&audit).is_empty());
    let cancellations = audit.records.lock().unwrap().clone();
    assert_eq!(cancellations.len(), 1);
    assert_eq!(cancellations[0].request().id(), &id);
    assert_eq!(cancellations[0].origin(), &CancellationOrigin::Runtime);
    let wire: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.path().join("permission-outcome")).unwrap(),
    )
    .unwrap();
    assert_eq!(wire["outcome"]["outcome"], "cancelled");
    close(&clock, &opened).await;
    assert_gone(&root, "pid");
}
