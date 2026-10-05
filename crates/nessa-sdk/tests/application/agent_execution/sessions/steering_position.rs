//! Where a steered message stands in its target turn, saved and read back:
//! both halves of the position or neither (A11), each path's bound on the
//! offset, the position as provider correlation evidence, and why a replayed
//! injection takes no bound of its own ("The app a message names",
//! `docs/design/mcp-app-calls.md`).
use super::super::test_support::{
    drawn, from, invocation_mut, mcp, snapshot, steered_log, text, tool_call, turn, DRAWN, STEERED,
};
use super::*;
use crate::application::agent_execution::{
    executions::{ExecutionEvent, ExecutionUpdate},
    permissions::ActionContext,
    sessions::{
        records::{continuation::Continuation, fold_changes},
        validation, InvocationSchedulingEvent, ProviderContext, SessionChange, SessionSnapshot,
    },
};
use crate::domain::agent_execution::executions::{
    InvocationKind, InvocationStage, SchedulingCause, SubmissionMode,
};

/// A saved steering position is both halves or neither (A11): a target
/// without its offset, or an offset without a target, is `Corrupt` on
/// restoration and on replay alike.
#[test]
fn a11_a_steering_target_and_offset_are_saved_together_or_not_at_all() {
    let log = steered_log(&[], DRAWN, text(), Vec::new(), Vec::new());
    let valid = fold_changes(None, &log).unwrap();
    for (id, offset, refusal) in [
        (STEERED, None, "steering target has no steering offset"),
        (DRAWN, Some(0), "targetless input has a steering offset"),
    ] {
        let refused = Err(StorageError::Corrupt(refusal.into()));
        let mut restored = valid.clone();
        invocation_mut(&mut restored, id).target_event_offset = offset;
        let mut replayed = log.clone();
        for change in &mut replayed {
            if let SessionChange::InputAccepted(record) = change {
                if record.request.execution_id.as_str() == id {
                    record.target_event_offset = offset;
                }
            }
        }
        assert_eq!(
            (
                validation::validate(&restored),
                fold_changes(None, &replayed).map(drop)
            ),
            (refused.clone(), refused),
            "restoration and replay, {id}"
        );
    }
}

/// Round 3's repro (A11 through A8): a steered snapshot whose offset was
/// removed, naming a call its target observed after the message, was
/// restored, as the offset bound was skipped. It is `Corrupt` now.
#[test]
fn a11_a_steered_snapshot_without_its_offset_cannot_name_a_later_call() {
    let shown = || tool_call("call-1").with_mcp_tool(mcp("charts", "show"));
    let mut restored = fold_changes(
        None,
        &steered_log(&[], DRAWN, text(), Vec::new(), vec![shown()]),
    )
    .unwrap();
    let steered = invocation_mut(&mut restored, STEERED);
    steered.request.user_message = from(drawn());
    steered.target_event_offset = None;
    assert_eq!(
        validation::validate(&restored),
        Err(StorageError::Corrupt(
            "steering target has no steering offset".into()
        ))
    );
}

/// Each path bounds a saved offset by the target history it holds. Replay's
/// `InputAccepted` holds the target's events so far, and takes exactly their
/// count: one short or one past it is `Corrupt`. Restoration holds the whole
/// target, and takes at most its count: one past it is `Corrupt`, and one
/// short is restored, as the target may have run on after the message.
#[test]
fn a_steering_offset_is_bounded_by_the_target_history_each_path_holds() {
    // The target observed one event before the message, and none after.
    let log = steered_log(&[], DRAWN, text(), vec![tool_call("plain")], Vec::new());
    let valid = fold_changes(None, &log).unwrap();
    for offset in [0, 2] {
        let mut replayed = log.clone();
        for change in &mut replayed {
            if let SessionChange::InputAccepted(record) = change {
                if record.request.execution_id.as_str() == STEERED {
                    record.target_event_offset = Some(offset);
                }
            }
        }
        assert_eq!(
            fold_changes(None, &replayed).map(drop),
            Err(StorageError::Corrupt(
                "steering offset is outside the prior target history".into()
            )),
            "replay, offset {offset}"
        );
    }
    let mut restored = valid.clone();
    invocation_mut(&mut restored, STEERED).target_event_offset = Some(2);
    assert_eq!(
        validation::validate(&restored),
        Err(StorageError::Corrupt(
            "steering offset is outside the preceding target history".into()
        )),
        "restoration, one past"
    );
    invocation_mut(&mut restored, STEERED).target_event_offset = Some(0);
    assert_eq!(
        validation::validate(&restored),
        Ok(()),
        "restoration, one short"
    );
}

/// A saved steering position is provider correlation evidence, read through
/// `SteeringPosition`: a message steered natively into a turn that has no
/// other provider evidence (an immediate turn still in flight) needs a
/// recorded provider context, on restoration and on replay alike.
#[test]
fn a_saved_steering_position_needs_a_recorded_provider_context() {
    let mut running = turn(DRAWN, text(), Vec::new());
    running.events.clear();
    running.result = None;
    running.local_outcome = None;
    let mut steered = running.clone();
    steered.request.execution_id = ExecutionId::new(STEERED).unwrap();
    steered.submission = SubmissionMode::Steering;
    steered.target_event_offset = Some(0);
    steered.scheduling = vec![InvocationSchedulingEvent {
        kind: InvocationKind::Steering,
        target: Some(ExecutionId::new(DRAWN).unwrap()),
        before: None,
        stage: InvocationStage::Queued,
        cause: SchedulingCause::Submitted,
        actor: Some(ActionContext::new("user", "test", "invoke").unwrap()),
    }];
    let mut restored = snapshot(vec![running, steered]);
    let judged = |restored: &SessionSnapshot| {
        let mut replayed = vec![SessionChange::Opened {
            id: restored.id.clone(),
            provider: restored.provider.clone(),
            context: restored.provider_context.clone(),
        }];
        replayed.extend(
            restored
                .invocations
                .iter()
                .cloned()
                .map(|record| SessionChange::InputAccepted(Box::new(record))),
        );
        (
            validation::validate(restored),
            fold_changes(None, &replayed).map(drop),
        )
    };
    assert_eq!(judged(&restored), (Ok(()), Ok(())), "recorded context");
    restored.provider_context = ProviderContext::Absent;
    let refused = Err(StorageError::Corrupt(
        "provider evidence requires a recorded provider context".into(),
    ));
    assert_eq!(
        judged(&restored),
        (refused.clone(), refused),
        "absent context"
    );
}

/// Why a replayed injection takes no offset bound of its own (the statement
/// in "The app a message names"): a message is admitted at offset `k` of its
/// target; a unit holding more of the target's observations then fails and is
/// taken back, last first. The target holds its `k` events again, never fewer,
/// and the injection replayed after it is accepted.
#[test]
fn a_replayed_injection_holds_after_a_failed_unit_takes_back_its_targets_later_events() {
    // The target observed one event before the message: `k` is 1.
    let offset = 1;
    let log = steered_log(&[], DRAWN, text(), vec![tool_call("plain")], Vec::new());
    let (admitted, injection) = log.split_at(log.len() - 1);
    assert!(matches!(
        injection,
        [SessionChange::SchedulingTransition { event, .. }]
            if event.stage == InvocationStage::Injected
    ));
    let mut continuation = Continuation::empty();
    continuation.apply_unit(admitted).unwrap();
    let target = ExecutionId::new(DRAWN).unwrap();
    let observed = |id| {
        SessionChange::ProviderObservation(ExecutionEvent::new(
            target.clone(),
            ExecutionUpdate::Tool(tool_call(id)),
        ))
    };
    let accepted_again = admitted
        .iter()
        .find(|change| {
            matches!(change, SessionChange::InputAccepted(record)
                if record.request.execution_id == target)
        })
        .cloned()
        .unwrap();
    // Both observations are staged, then the unit fails on its last change.
    assert_eq!(
        continuation.apply_unit(&[observed("more"), observed("again"), accepted_again]),
        Err(StorageError::Corrupt(
            "execution identity was accepted twice".into()
        ))
    );
    let events = |continuation: &Continuation| {
        continuation
            .snapshot
            .as_ref()
            .unwrap()
            .invocations
            .iter()
            .find(|record| record.request.execution_id == target)
            .unwrap()
            .events
            .len()
    };
    assert!(events(&continuation) >= offset, "the target lost an event");
    assert_eq!(events(&continuation), offset, "the failed unit's events");
    assert_eq!(continuation.apply_unit(injection), Ok(()));
}
