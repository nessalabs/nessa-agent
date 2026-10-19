//! An unanswered review settles when its deadline passes, and the turn goes on.
use super::super::support::*;
use crate::infrastructure::clock::Clock;
use std::task::{Context, Waker};

const DENY_REASON: &str = STANDARD_DENY_REASON;
const ALLOW_REASON: &str = STANDARD_ALLOW_REASON;

fn recorded(text: &str) -> DeadlineReason {
    DeadlineReason::new(text).unwrap()
}

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
    let wait_ms = crate::domain::agent_execution::permissions::wait_ms(deadline.wait());
    assert_eq!(
        deadline.due_at_ms(),
        Some(clock.unix_ms_at(clock.now()).saturating_add(wait_ms)),
        "dueAtMs is the injected clock plus the wait"
    );
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
    next_until_finished(opened, "the turn").await
}

/// Like `until_finished`, but a closed session names `what` instead of
/// unwrapping an empty event stream.
async fn next_until_finished(
    opened: &mut OpenedProviderSession,
    what: &str,
) -> Vec<ExecutionUpdate> {
    let mut updates = Vec::new();
    loop {
        let update = match timeout(Duration::from_secs(3), opened.events.next()).await {
            Ok(Ok(Some(event))) => event.into_update(),
            Ok(Ok(None)) | Ok(Err(_)) => panic!("{what}: the session closed"),
            Err(_) => panic!("{what}: no update arrived"),
        };
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
        key: None,
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
                reason: recorded(DENY_REASON),
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
    for mode in ["permission-read-deadline", "permission-read-sparse"] {
        a_read_is_allowed_at_its_deadline(mode).await;
    }
}

// `permission-read-sparse` names only the tool on the permission frame; the
// read is judged on what the earlier tool_call said about it.
async fn a_read_is_allowed_at_its_deadline(mode: &str) {
    let mut review = pending(mode).await;
    assert_eq!(
        review.deadline.class(),
        PermissionToolClass::ReadOnly,
        "{mode}"
    );
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
                reason: recorded(ALLOW_REASON),
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
async fn a_wall_clock_no_client_can_read_leaves_the_review_to_a_person() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, mut config, model) = test_acp_configuration("permission", 16);
    let clock = Arc::new(ManualClock::at_unix_ms(MAX_DEADLINE_DUE_MS));
    config.clock = clock.clone();
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
    let ExecutionUpdate::PermissionRequested { id, deadline, .. } = next(&mut opened).await else {
        panic!("expected permission");
    };
    assert_eq!(
        deadline, None,
        "no card may promise a moment it cannot show"
    );

    clock.advance(Duration::from_secs(60));
    let late = timeout(Duration::from_millis(400), opened.events.next()).await;
    assert!(
        late.is_err(),
        "an unpublished deadline must not decide the review"
    );
    opened
        .session
        .answer_permission(human_allow(id))
        .await
        .unwrap();
    let updates = until_finished(&mut opened).await;
    assert!(updates
        .iter()
        .all(|update| !matches!(update, ExecutionUpdate::DeadlineAnswered { .. })));
    assert_eq!(
        promptly(active).await.unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    assert!(answered(&audit).iter().all(|record| matches!(
        record.resolution().attribution().basis(),
        ApprovalBasis::Explicit
    )));
    close(&clock, &opened).await;
    assert_gone(&root, "pid");
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

#[tokio::test(flavor = "current_thread")]
async fn an_unrelated_command_already_queued_does_not_hold_a_due_deadline() {
    let _slot = process_test_slot().await;
    // The provider has left a frame unfinished. Dispatching another prompt
    // drains that output before it returns, so it cannot return until the
    // review is answered. Queuing the prompt and reaching the deadline in
    // one wait must still settle the deadline.
    let mut review = pending("permission-unfinished-frame").await;
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

    // Queue another prompt without letting the worker run, then reach the
    // deadline. The prompt is not an answer to the due review. Taking it
    // first waits on the unfinished frame and never settles the deadline.
    let mut second = review
        .opened
        .session
        .execute(prompt("later"), Arc::new(|| Ok(())));
    let mut context = Context::from_waker(Waker::noop());
    assert!(second.as_mut().poll(&mut context).is_pending());
    review.clock.advance(Duration::from_secs(30));

    let mut saw_deadline = false;
    let updates = timeout(Duration::from_secs(5), async {
        let mut updates = Vec::new();
        loop {
            let update = review
                .opened
                .events
                .next()
                .await
                .expect("the event stream stayed open")
                .expect("the session stayed open")
                .into_update();
            let finished = matches!(update, ExecutionUpdate::Finished(_));
            saw_deadline |= matches!(update, ExecutionUpdate::DeadlineAnswered { .. });
            updates.push(update);
            if finished {
                return updates;
            }
        }
    })
    .await
    .expect("a queued prompt held the deadline");
    assert!(
        saw_deadline,
        "a queued prompt held the deadline: {updates:?}"
    );
    drop(second);
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

// Two answers to the due review are queued as the deadline passes. The
// first fails its audit, which ends the drive loop. The second was admitted
// too, so teardown still answers it: it is told the review is already
// decided, not that the session went away.
#[tokio::test(flavor = "current_thread")]
async fn an_admitted_command_outlives_a_deadline_batch_that_stops() {
    let _slot = process_test_slot().await;
    let (root, mut config, model) = test_acp_configuration("permission", 16);
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
    let _active = start(&opened, "write").await;
    assert!(matches!(next(&mut opened).await, ExecutionUpdate::Tool(_)));
    let ExecutionUpdate::PermissionRequested { id, .. } = next(&mut opened).await else {
        panic!("expected permission");
    };
    timeout(Duration::from_secs(5), async {
        while !clock
            .waits()
            .iter()
            .any(|wait| wait.limit() == Duration::from_secs(30))
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the review's deadline was never armed");

    let mut context = Context::from_waker(Waker::noop());
    let mut first = opened.session.answer_permission(human_allow(id.clone()));
    assert!(first.as_mut().poll(&mut context).is_pending());
    let mut second = opened.session.answer_permission(human_allow(id));
    assert!(second.as_mut().poll(&mut context).is_pending());
    clock.advance(Duration::from_secs(30));

    let first = timeout(Duration::from_secs(5), first)
        .await
        .expect("the first answer was not taken")
        .unwrap_err();
    assert_eq!(first.error(), &AgentError::AuditFailure);
    let second = timeout(Duration::from_secs(5), second)
        .await
        .expect("the second answer was never answered")
        .unwrap_err();
    assert_eq!(
        second.permission_selection(),
        Some(PermissionSelectionState::Pending),
        "the admitted answer was dropped: {second:?}"
    );
    assert_ne!(second.error(), &AgentError::Closed, "{second:?}");
    let closing = opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()));
    let _ = promptly(clock.passing(a_grace, closing)).await;
    assert_gone(&root, "pid");
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
                && matches!(cancellation.origin(), CancellationOrigin::Deadline(by) if by.decides_as(&deadline))
    ));
    assert!(updates.iter().any(|update| matches!(
        update,
        ExecutionUpdate::DeadlineAnswered {
            effect: PermissionEffect::Deny,
            reason,
            ..
        } if reason.as_str() == deadline.reason()
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
    assert!(
        matches!(cancellations[0].origin(), CancellationOrigin::Deadline(by) if by.decides_as(&deadline)),
        "the audit names the deadline as the decider: {:?}",
        cancellations[0].origin()
    );
    let wire: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.path().join("permission-outcome")).unwrap(),
    )
    .unwrap();
    assert_eq!(wire["outcome"]["outcome"], "cancelled");
    close(&clock, &opened).await;
    assert_gone(&root, "pid");
}

async fn quiet_after(clock: &ManualClock, opened: &mut OpenedProviderSession) {
    clock.advance(Duration::from_secs(30));
    let late = timeout(Duration::from_millis(400), opened.events.next()).await;
    match late {
        Err(_) | Ok(Ok(None)) | Ok(Err(_)) => {}
        Ok(Ok(Some(event))) => assert!(
            !matches!(event.update(), ExecutionUpdate::DeadlineAnswered { .. }),
            "a deadline must not answer after the turn is over: {event:?}"
        ),
    }
}

fn deadline_answers(audit: &RecordingAudit) -> Vec<PermissionAnswerRecord> {
    answered(audit)
        .into_iter()
        .filter(|record| {
            matches!(
                record.resolution().attribution().basis(),
                ApprovalBasis::Deadline(_)
            )
        })
        .collect()
}

/// The next event, if one arrives while the session is still open.
async fn next_while_open(opened: &mut OpenedProviderSession) -> Option<ExecutionEvent> {
    match timeout(Duration::from_millis(400), opened.events.next()).await {
        Err(_) => None,
        Ok(Ok(None)) | Ok(Err(_)) => {
            panic!("the session ended while a deadline could still answer")
        }
        Ok(Ok(Some(event))) => Some(event),
    }
}

async fn appeared(path: &std::path::Path) {
    let started = std::time::Instant::now();
    while !path.exists() {
        if started.elapsed() > Duration::from_secs(5) {
            panic!("missing {}", path.display());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn assert_no_deadline_for(id: &PermissionId, opened: &mut OpenedProviderSession) {
    loop {
        let Some(event) = next_while_open(opened).await else {
            return;
        };
        if matches!(event.update(), ExecutionUpdate::PermissionCancelled(_)) {
            continue;
        }
        assert!(
            !matches!(event.update(), ExecutionUpdate::DeadlineAnswered { id: answered, .. } if answered == id),
            "the deadline answered {id:?}: {event:?}"
        );
        panic!("unexpected event after the deadline: {event:?}");
    }
}

#[tokio::test]
async fn a_turn_that_ends_with_a_request_pending_does_not_answer_it_later() {
    let _slot = process_test_slot().await;
    let mut review = pending("permission-finished").await;
    let first_id = review.id.clone();
    let updates = until_finished(&mut review.opened).await;
    assert!(
        updates
            .iter()
            .any(|update| matches!(update, ExecutionUpdate::Finished(_))),
        "{updates:?}"
    );
    assert!(updates.iter().all(|update| !matches!(
        update,
        ExecutionUpdate::DeadlineAnswered { id, .. } if id == &first_id
    )));
    // Turn 2 starts at once, while turn 1's deadline is still in the future,
    // and stays open. Advancing past that deadline must not answer turn 1.
    let second = start(&review.opened, "second").await;
    match next(&mut review.opened).await {
        ExecutionUpdate::Message(chunk) if chunk.as_str() == "turn-2-open" => {}
        update => panic!("expected turn 2 to be open, got {update:?}"),
    }
    review.clock.advance(Duration::from_secs(30));
    assert_no_deadline_for(&first_id, &mut review.opened).await;
    assert!(
        deadline_answers(&review.audit).is_empty(),
        "turn 1's deadline wrote an audit record"
    );
    std::fs::write(review.root.path().join("release-turn-2"), "yes").unwrap();
    let second_updates = until_finished(&mut review.opened).await;
    assert!(second_updates.iter().all(|update| !matches!(
        update,
        ExecutionUpdate::DeadlineAnswered { id, .. } if id == &first_id
    )));
    assert!(
        deadline_answers(&review.audit).is_empty(),
        "turn 1's deadline wrote an audit record"
    );
    assert_eq!(
        promptly(review.active).await.unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    assert_eq!(
        promptly(second).await.unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    close(&review.clock, &review.opened).await;
    assert_gone(&review.root, "pid");
}

#[tokio::test]
async fn a_provider_cancelled_turn_forgets_the_armed_deadline() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, mut config, model) =
        test_acp_configuration("permission-deadline-provider-cancel", 16);
    config.execution_timeout = None;
    // Longer than the review's 30 second wait, so the grace outlasts it and
    // the provider is still alive when that wait runs out.
    config.shutdown_grace = Duration::from_secs(60);
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
    let id = loop {
        match next(&mut opened).await {
            ExecutionUpdate::Tool(_) => {}
            ExecutionUpdate::PermissionRequested { id, deadline, .. } => {
                let deadline = deadline.expect("the open review is armed");
                assert!(deadline.armed());
                break id;
            }
            update => panic!("expected permission, got {update:?}"),
        }
    };
    std::fs::write(root.path().join("release-provider-cancel"), "yes").unwrap();
    appeared(&root.path().join("provider-cancel-sent")).await;
    // Teardown has sent session/cancel and is inside the grace. The provider
    // is held there. Finished(Cancelled) is published when that hold ends.
    appeared(&root.path().join("cancel-observed")).await;
    clock.advance(Duration::from_secs(30));
    assert_no_deadline_for(&id, &mut opened).await;
    assert!(
        deadline_answers(&audit).is_empty(),
        "a provider-cancelled turn wrote a deadline audit record"
    );
    std::fs::write(root.path().join("release-close"), "yes").unwrap();
    let updates = until_finished(&mut opened).await;
    assert!(
        updates
            .iter()
            .all(|update| !matches!(update, ExecutionUpdate::DeadlineAnswered { .. })),
        "{updates:?}"
    );
    assert!(
        matches!(
            updates.last(),
            Some(ExecutionUpdate::Finished(ExecutionOutcome::Cancelled))
        ),
        "provider cancel tears the session down: {updates:?}"
    );
    assert_eq!(
        promptly(active).await.unwrap(),
        Ok(ExecutionOutcome::Cancelled)
    );
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn close_during_a_pending_request_forgets_the_deadline() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, mut config, model) = test_acp_configuration("permission-deadline-close-hold", 16);
    config.execution_timeout = None;
    // Longer than the review's 30 second wait, so the grace outlasts it and
    // the provider is still alive when that wait runs out.
    config.shutdown_grace = Duration::from_secs(60);
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
    let id = loop {
        match next(&mut opened).await {
            ExecutionUpdate::Tool(_) => {}
            ExecutionUpdate::PermissionRequested { id, deadline, .. } => {
                let deadline = deadline.expect("the open review is armed");
                assert!(deadline.armed());
                assert_eq!(
                    deadline.due_at_ms(),
                    Some(
                        clock
                            .unix_ms_at(clock.now())
                            .saturating_add(wait_ms(deadline.wait()))
                    )
                );
                break id;
            }
            update => panic!("expected permission, got {update:?}"),
        }
    };
    let session = opened.session.clone();
    let closing = session.shutdown(SessionCloseRequest::Explicit(close_action()));
    tokio::pin!(closing);
    let observed = root.path().join("cancel-observed");
    promptly(async {
        loop {
            if observed.exists() {
                break;
            }
            tokio::select! {
                biased;
                done = &mut closing => panic!("closed before the provider saw cancel: {done:?}"),
                _ = tokio::time::sleep(Duration::from_millis(10)) => {}
            }
        }
    })
    .await;
    clock.advance(Duration::from_secs(30));
    assert_no_deadline_for(&id, &mut opened).await;
    assert!(
        deadline_answers(&audit).is_empty(),
        "close wrote a deadline audit record during the grace"
    );
    std::fs::write(root.path().join("release-close"), "yes").unwrap();
    let closed = promptly(closing).await;
    closed.into_result().unwrap();
    assert_eq!(
        promptly(active).await.unwrap(),
        Ok(ExecutionOutcome::Cancelled)
    );
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn stopping_a_turn_during_a_pending_request_holds_the_next_prompt() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, mut config, model) = test_acp_configuration("permission-deadline-stop", 16);
    config.execution_timeout = None;
    // Longer than the pending review's wait, so running that wait out does not
    // also expire the stopped prompt's grace.
    config.shutdown_grace = Duration::from_secs(60);
    config.refusal = ConfiguredRefusal::stopping(StopKind::EndTurn, super::stopping_rule());
    let clock = manual_clock(&mut config);
    let binding =
        ClaudeAcpProvider::new(config, &model, TokenLimits::new(900, 100).unwrap(), audit).unwrap();
    let mut opened = super::open_that_stops(&binding, Arc::new(RecordingAudit::default())).await;
    let first = start(&opened, "write").await;
    let mut armed = false;
    let mut declined = false;
    let mut finished = false;
    for _ in 0..12 {
        match next(&mut opened).await {
            ExecutionUpdate::PermissionRequested { deadline, .. } => {
                let deadline = deadline.expect("the open review is armed");
                assert!(deadline.armed());
                assert_eq!(deadline.wait(), Duration::from_secs(30));
                armed = true;
            }
            ExecutionUpdate::ReviewDeclined(_) => declined = true,
            ExecutionUpdate::DeadlineAnswered { .. } => {
                panic!("the deadline answered a turn that was stopping")
            }
            ExecutionUpdate::Finished(ExecutionOutcome::Cancelled) => {
                finished = true;
                break;
            }
            _ => {}
        }
    }
    assert!(armed, "the write was not left pending");
    assert!(declined, "the shell was not the stop");
    assert!(finished, "the stopped turn did not finish");
    let second = start(&opened, "second-turn").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !second.is_finished(),
        "the next prompt ran before the stopped reply"
    );
    clock.advance(Duration::from_secs(30));
    let late = timeout(Duration::from_millis(400), opened.events.next()).await;
    match late {
        Err(_) | Ok(Ok(None)) | Ok(Err(_)) => {}
        Ok(Ok(Some(event))) => assert!(
            !matches!(event.update(), ExecutionUpdate::DeadlineAnswered { .. }),
            "a stopped turn's deadline answered during the hold: {event:?}"
        ),
    }
    assert!(
        !second.is_finished(),
        "the held prompt ran when the clock did"
    );
    std::fs::write(root.path().join("release-stopped-result"), "yes").unwrap();
    assert_eq!(
        promptly(first).await.unwrap(),
        Ok(ExecutionOutcome::Cancelled)
    );
    let second_updates = until_finished(&mut opened).await;
    assert!(second_updates
        .iter()
        .all(|update| !matches!(update, ExecutionUpdate::DeadlineAnswered { .. })));
    assert_eq!(
        promptly(second).await.unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    quiet_after(&clock, &mut opened).await;
    close(&clock, &opened).await;
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn a_read_outside_the_workspace_is_denied_and_one_inside_is_allowed() {
    let _slot = process_test_slot().await;
    for (mode, class, wait, effect, sentence) in [
        (
            "permission-read-outside",
            PermissionToolClass::Other,
            Duration::from_secs(30),
            PermissionEffect::Deny,
            DENY_REASON,
        ),
        (
            "permission-read-symlink",
            PermissionToolClass::Other,
            Duration::from_secs(30),
            PermissionEffect::Deny,
            DENY_REASON,
        ),
        (
            "permission-read-no-path",
            PermissionToolClass::Other,
            Duration::from_secs(30),
            PermissionEffect::Deny,
            DENY_REASON,
        ),
    ] {
        let review = pending(mode).await;
        assert_eq!(review.deadline.class(), class, "{mode}");
        assert_eq!(review.deadline.wait(), wait, "{mode}");
        assert_eq!(review.deadline.effect(), effect, "{mode}");
        assert_eq!(review.deadline.reason(), sentence, "{mode}");
        assert!(review.deadline.armed(), "{mode}");
        assert!(review.deadline.due_at_ms().is_some(), "{mode}");
        close(&review.clock, &review.opened).await;
        assert_gone(&review.root, "pid");
    }
}

async fn both_pending(mode: &str) -> PendingReview {
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding, clock) = test_acp_binding_on_clock(mode, audit.clone(), None);
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "write").await;
    let mut permissions = 0;
    let mut id = PermissionId::new("pending").unwrap();
    let mut deadline = None;
    while permissions < 2 {
        match next(&mut opened).await {
            ExecutionUpdate::Tool(_) => {}
            ExecutionUpdate::PermissionRequested {
                id: requested,
                deadline: armed,
                ..
            } => {
                id = requested;
                deadline = Some(armed.expect("each review is armed"));
                permissions += 1;
            }
            update => panic!("{mode}: expected the two reviews, got {update:?}"),
        }
    }
    PendingReview {
        root,
        opened,
        active,
        clock,
        audit,
        id,
        deadline: deadline.expect("two reviews"),
    }
}

#[tokio::test]
async fn two_reviews_settle_in_deadline_order_then_id_order() {
    let _slot = process_test_slot().await;
    let mut review = both_pending("permission-deadline-pair").await;
    review.clock.advance(Duration::from_secs(30));
    let updates = until_finished(&mut review.opened).await;
    let answered: Vec<_> = updates
        .iter()
        .filter_map(|update| match update {
            ExecutionUpdate::DeadlineAnswered { id, effect, .. } => {
                Some((id.as_str().to_owned(), *effect))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        answered,
        vec![
            ("2".into(), PermissionEffect::Allow),
            ("1".into(), PermissionEffect::Deny),
        ],
        "the earlier deadline settles first even though its id sorts later: {updates:?}"
    );
    assert!(updates.iter().any(|update| matches!(
        update,
        ExecutionUpdate::Finished(ExecutionOutcome::Completed)
    )));
    close(&review.clock, &review.opened).await;
    assert_gone(&review.root, "pid");

    let mut same = both_pending("permission-deadline-same").await;
    same.clock.advance(Duration::from_secs(30));
    let updates = until_finished(&mut same.opened).await;
    let ids: Vec<_> = updates
        .iter()
        .filter_map(|update| match update {
            ExecutionUpdate::DeadlineAnswered { id, .. } => Some(id.as_str().to_owned()),
            _ => None,
        })
        .collect();
    assert_eq!(ids, vec!["1".to_owned(), "2".to_owned()], "{updates:?}");
    close(&same.clock, &same.opened).await;
    assert_gone(&same.root, "pid");
}

/// Records like `RecordingAudit`, and moves the clock the first time an
/// answer is recorded: time passing while an audit write is awaited.
struct SlowFirstAnswer {
    inner: RecordingAudit,
    clock: std::sync::OnceLock<Arc<ManualClock>>,
    by: Duration,
    moved: std::sync::atomic::AtomicBool,
}
impl ExecutionAudit for SlowFirstAnswer {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        if matches!(record, ExecutionAuditRecord::Answered(_))
            && !self.moved.swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            self.clock.get().expect("clock set").advance(self.by);
        }
        self.inner.record(record)
    }
}

// The read (8 s) falls due with answers to it and to the write (30 s)
// already queued. Recording the read's answer takes until the write is due
// too. The write's answer was admitted before its deadline, so it decides
// the write, not the deadline.
#[tokio::test(flavor = "current_thread")]
async fn a_review_due_during_another_settlement_keeps_its_queued_answer() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(SlowFirstAnswer {
        inner: RecordingAudit::default(),
        clock: std::sync::OnceLock::new(),
        by: Duration::from_secs(22),
        moved: std::sync::atomic::AtomicBool::new(false),
    });
    let (root, mut config, model) = test_acp_configuration("permission-deadline-pair", 16);
    config.execution_timeout = None;
    let clock = manual_clock(&mut config);
    assert!(audit.clock.set(clock.clone()).is_ok());
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
    let _active = start(&opened, "write").await;
    let mut permissions = 0;
    while permissions < 2 {
        if let ExecutionUpdate::PermissionRequested { .. } = next(&mut opened).await {
            permissions += 1;
        }
    }
    timeout(Duration::from_secs(5), async {
        while !clock
            .waits()
            .iter()
            .any(|wait| wait.limit() == Duration::from_secs(8))
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the read's deadline was never armed");

    let (write, read) = (
        PermissionId::new("1").unwrap(),
        PermissionId::new("2").unwrap(),
    );
    let mut context = Context::from_waker(Waker::noop());
    let mut read_answer = opened.session.answer_permission(human_allow(read));
    assert!(read_answer.as_mut().poll(&mut context).is_pending());
    let mut deny = human_allow(write.clone());
    deny.option_id = PermissionOptionId::new("deny-one").unwrap();
    let mut write_answer = opened.session.answer_permission(deny);
    assert!(write_answer.as_mut().poll(&mut context).is_pending());
    clock.advance(Duration::from_secs(8));

    timeout(Duration::from_secs(5), read_answer)
        .await
        .expect("the read's answer was not taken")
        .unwrap();
    let decided = timeout(Duration::from_secs(5), write_answer)
        .await
        .expect("the write's answer was not taken");
    assert!(
        decided.is_ok(),
        "the deadline took the write first: {decided:?}"
    );
    assert!(answered(&audit.inner).iter().all(|record| matches!(
        record.resolution().attribution().basis(),
        ApprovalBasis::Explicit
    )));
    let closing = opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()));
    let _ = promptly(clock.passing(a_grace, closing)).await;
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn codex_prefers_decline_and_says_when_only_cancel_is_offered() {
    let _slot = process_test_slot().await;
    for (mode, option, sentence, stops) in [
        ("deadline-decline", "decline", DENY_REASON, false),
        ("deadline-cancel-only", "cancel", STANDARD_STOP_REASON, true),
    ] {
        let audit = Arc::new(RecordingAudit::default());
        let (root, mut config, model) = codex_configuration(mode, 16);
        config.execution_timeout = None;
        let clock = manual_clock(&mut config);
        let binding = CodexAcpProvider::new(
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
        let deadline = loop {
            match next(&mut opened).await {
                ExecutionUpdate::Tool(_) => {}
                ExecutionUpdate::PermissionRequested { deadline, .. } => {
                    break deadline.expect("codex review is armed")
                }
                update => panic!("{mode}: expected permission, got {update:?}"),
            }
        };
        assert_eq!(deadline.stops_turn(), stops, "{mode}");
        assert_eq!(deadline.reason(), sentence, "{mode}");
        assert!(deadline.armed(), "{mode}");
        assert_eq!(
            deadline.due_at_ms(),
            Some(
                clock
                    .unix_ms_at(clock.now())
                    .saturating_add(wait_ms(deadline.wait()))
            ),
            "{mode}"
        );
        clock
            .run_out(|wait| wait.limit() == Duration::from_secs(30))
            .await;
        let updates = until_finished(&mut opened).await;
        assert!(
            updates.iter().any(|update| matches!(
                update,
                ExecutionUpdate::DeadlineAnswered { reason, .. } if reason.as_str() == sentence
            )),
            "{mode}: {updates:?}"
        );
        let outcome = if stops {
            ExecutionOutcome::Cancelled
        } else {
            ExecutionOutcome::Completed
        };
        if stops {
            assert!(
                updates
                    .iter()
                    .all(|update| !matches!(update, ExecutionUpdate::Message(_))),
                "cancel stopped the turn, so it does not keep speaking: {updates:?}"
            );
            assert!(
                matches!(
                    updates.last(),
                    Some(ExecutionUpdate::Finished(ExecutionOutcome::Cancelled))
                ),
                "cancel stops the turn: {updates:?}"
            );
        }
        assert_eq!(promptly(active).await.unwrap(), Ok(outcome), "{mode}");
        if stops {
            let again = start(&opened, "again").await;
            let second =
                next_until_finished(&mut opened, "the session stayed open after cancel").await;
            assert!(
                second.iter().any(|update| matches!(
                    update,
                    ExecutionUpdate::Message(chunk) if chunk.as_str() == "turn-2-open"
                )),
                "the session stayed open after cancel: {second:?}"
            );
            assert!(
                matches!(
                    second.last(),
                    Some(ExecutionUpdate::Finished(ExecutionOutcome::Completed))
                ),
                "a later turn completes in the same session: {second:?}"
            );
            assert_eq!(
                promptly(again).await.unwrap(),
                Ok(ExecutionOutcome::Completed),
                "{mode}: the session stayed open"
            );
        }
        let wire: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.path().join("permission-outcome")).unwrap(),
        )
        .unwrap();
        assert_eq!(wire["outcome"]["optionId"], option, "{mode}");
        assert_eq!(wire["_meta"]["deadlineReason"], sentence, "{mode}");
        close(&clock, &opened).await;
        assert_gone(&root, "pid");
    }
}

#[tokio::test]
async fn a_person_choosing_codex_cancel_ends_the_turn_and_leaves_the_session_open() {
    let _slot = process_test_slot().await;
    let (root, mut config, model) = codex_configuration("deadline-cancel-only", 16);
    config.execution_timeout = None;
    let clock = manual_clock(&mut config);
    let binding = CodexAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "write").await;
    let id = loop {
        match next(&mut opened).await {
            ExecutionUpdate::Tool(_) => {}
            ExecutionUpdate::PermissionRequested { id, .. } => break id,
            update => panic!("expected permission, got {update:?}"),
        }
    };
    opened
        .session
        .answer_permission(PermissionAnswer {
            key: None,
            attribution: ApprovalAttribution::new(
                ActionContext::new("reviewer", "nessa.panel", "human-answer").unwrap(),
                ApprovalBasis::Explicit,
            ),
            execution_id: ExecutionId::new("write").unwrap(),
            id,
            option_id: PermissionOptionId::new("cancel").unwrap(),
        })
        .await
        .map_err(|failure| failure.into_error())
        .unwrap();
    let updates = until_finished(&mut opened).await;
    assert!(
        matches!(
            updates.last(),
            Some(ExecutionUpdate::Finished(ExecutionOutcome::Cancelled))
        ),
        "a hand-picked cancel ends the turn: {updates:?}"
    );
    assert_eq!(
        promptly(active).await.unwrap(),
        Ok(ExecutionOutcome::Cancelled)
    );
    let again = start(&opened, "again").await;
    let second = next_until_finished(
        &mut opened,
        "the session stayed open after a hand-picked cancel",
    )
    .await;
    assert!(
        second.iter().any(|update| matches!(
            update,
            ExecutionUpdate::Message(chunk) if chunk.as_str() == "turn-2-open"
        )),
        "the session stayed open after a hand-picked cancel: {second:?}"
    );
    assert!(
        matches!(
            second.last(),
            Some(ExecutionUpdate::Finished(ExecutionOutcome::Completed))
        ),
        "a later turn completes in the same session: {second:?}"
    );
    assert_eq!(
        promptly(again).await.unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    close(&clock, &opened).await;
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn a_person_choosing_codex_decline_then_an_unasked_cancel_ends_the_session() {
    let _slot = process_test_slot().await;
    let (root, mut config, model) = codex_configuration("deadline-decline-unasked-cancel", 16);
    config.execution_timeout = None;
    let binding = CodexAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "write").await;
    let id = loop {
        match next(&mut opened).await {
            ExecutionUpdate::Tool(_) => {}
            ExecutionUpdate::PermissionRequested { id, .. } => break id,
            update => panic!("expected permission, got {update:?}"),
        }
    };
    opened
        .session
        .answer_permission(PermissionAnswer {
            key: None,
            attribution: ApprovalAttribution::new(
                ActionContext::new("reviewer", "nessa.panel", "human-answer").unwrap(),
                ApprovalBasis::Explicit,
            ),
            execution_id: ExecutionId::new("write").unwrap(),
            id,
            option_id: PermissionOptionId::new("decline").unwrap(),
        })
        .await
        .map_err(|failure| failure.into_error())
        .unwrap();
    let updates = until_finished(&mut opened).await;
    assert!(
        matches!(
            updates.last(),
            Some(ExecutionUpdate::Finished(ExecutionOutcome::Cancelled))
        ),
        "an unasked cancel after decline ends the session: {updates:?}"
    );
    assert_eq!(
        promptly(active).await.unwrap(),
        Ok(ExecutionOutcome::Cancelled)
    );
    match timeout(Duration::from_secs(3), opened.events.next()).await {
        Ok(Ok(None)) | Ok(Err(_)) => {}
        other => panic!(
            "an unasked cancel after decline ended the session: the session stayed open: {other:?}"
        ),
    }
    let wire: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.path().join("permission-outcome")).unwrap(),
    )
    .unwrap();
    assert_eq!(wire["outcome"]["optionId"], "decline");
    assert_gone(&root, "pid");
}

// Only a refusal stops the turn. An allow that carries Codex's stopping id
// is still an allow, so the cancel after it was not asked for.
#[tokio::test]
async fn an_allow_named_cancel_then_an_unasked_cancel_ends_the_session() {
    let _slot = process_test_slot().await;
    let (root, mut config, model) = codex_configuration("deadline-allow-named-cancel", 16);
    config.execution_timeout = None;
    let binding = CodexAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "write").await;
    let id = loop {
        match next(&mut opened).await {
            ExecutionUpdate::Tool(_) => {}
            ExecutionUpdate::PermissionRequested { id, .. } => break id,
            update => panic!("expected permission, got {update:?}"),
        }
    };
    opened
        .session
        .answer_permission(PermissionAnswer {
            key: None,
            attribution: ApprovalAttribution::new(
                ActionContext::new("reviewer", "nessa.panel", "human-answer").unwrap(),
                ApprovalBasis::Explicit,
            ),
            execution_id: ExecutionId::new("write").unwrap(),
            id,
            option_id: PermissionOptionId::new("cancel").unwrap(),
        })
        .await
        .map_err(|failure| failure.into_error())
        .unwrap();
    let updates = until_finished(&mut opened).await;
    assert!(
        matches!(
            updates.last(),
            Some(ExecutionUpdate::Finished(ExecutionOutcome::Cancelled))
        ),
        "an unasked cancel after an allow named cancel ends the session: {updates:?}"
    );
    assert_eq!(
        promptly(active).await.unwrap(),
        Ok(ExecutionOutcome::Cancelled)
    );
    match timeout(Duration::from_secs(3), opened.events.next()).await {
        Ok(Ok(None)) | Ok(Err(_)) => {}
        other => panic!(
            "an unasked cancel after an allow named cancel ended the session: the session stayed open: {other:?}"
        ),
    }
    let wire: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.path().join("permission-outcome")).unwrap(),
    )
    .unwrap();
    assert_eq!(wire["outcome"]["optionId"], "cancel");
    assert_gone(&root, "pid");
}
