//! The app a message names, row by row of "The app a message names"
//! (`docs/design/mcp-app-calls.md`): the rule as admission asks it of the
//! turns saved before the message, as restoration asks it of a snapshot, and
//! as a replayed record log asks it of each accepted input.
use super::*;
use crate::application::agent_execution::sessions::{
    InvocationSchedulingEvent, QueueHistoryRecord,
};
use crate::application::agent_execution::{
    executions::{ExecutionEvent, ExecutionRequest},
    permissions::ActionContext,
    providers::ProviderIdentity,
    sessions::{
        records::fold_changes, validation, ProviderContext, SessionChange, SessionSnapshot,
        StorageError, SubmissionAcknowledgement,
    },
};
use crate::domain::agent_execution::{
    executions::{
        ExecutionOutcome, InvocationKind, InvocationStage, QueueMutation, SchedulingCause,
        SubmissionMode,
    },
    prompts::PromptText,
    sessions::{ExecutionSessionId, SessionId},
    tools::ToolCallUpdate,
};

/// The turn whose tool call `call-1` was to `charts/show`, beside a tool call
/// `plain` that named no MCP server.
const DRAWN: &str = "turn-1";

fn mcp(server: &str, tool: &str) -> McpTool {
    McpTool::new(server, tool).unwrap()
}
fn app(turn: &str, tool_id: &str, tool: McpTool) -> McpAppSource {
    McpAppSource::new(
        ExecutionId::new(turn).unwrap(),
        ToolCallId::new(tool_id).unwrap(),
        tool,
    )
    .unwrap()
}
fn drawn() -> McpAppSource {
    app(DRAWN, "call-1", mcp("charts", "show"))
}
fn text() -> UserMessage {
    UserMessage::text_only(PromptText::new("plot").unwrap())
}
fn from(app: McpAppSource) -> UserMessage {
    text().sent_by(MessageSender::App(app))
}
fn carrying(apps: impl IntoIterator<Item = McpAppSource>) -> UserMessage {
    text()
        .with_app_model_context(
            apps.into_iter()
                .enumerate()
                .map(|(index, app)| {
                    AppModelContext::new(app, &format!("update-{index}"), Some("x".into()), None)
                        .unwrap()
                        .unwrap()
                })
                .collect(),
        )
        .unwrap()
}
/// A completed turn `id` whose message is `message` and whose tool calls
/// are `tools`, observed in that order.
fn turn(id: &str, message: UserMessage, tools: Vec<ToolCallUpdate>) -> InvocationRecord {
    let id = ExecutionId::new(id).unwrap();
    let mut events: Vec<_> = tools
        .into_iter()
        .map(|update| ExecutionEvent::new(id.clone(), ExecutionUpdate::Tool(update)))
        .collect();
    events.push(ExecutionEvent::new(
        id.clone(),
        ExecutionUpdate::Finished(ExecutionOutcome::Completed),
    ));
    InvocationRecord {
        target_event_offset: None,
        submission: SubmissionMode::Immediate,
        request: ExecutionRequest {
            execution_id: id,
            user_message: message,
            estimated_input_tokens: 1,
            reserved_output_tokens: 1,
        },
        actor: ActionContext::new("user", "test", "invoke").unwrap(),
        acknowledgement: SubmissionAcknowledgement::Pending,
        events,
        scheduling: Vec::new(),
        provider_report: None,
        local_cancellation: None,
        local_outcome: Some(ExecutionOutcome::Completed),
        cancellation: None,
        result: Some(Ok(ExecutionOutcome::Completed)),
    }
}
fn tool_call(id: &str) -> ToolCallUpdate {
    ToolCallUpdate::new(ToolCallId::new(id).unwrap(), None, None, None, None, None)
}
/// The turn that drew the app: `call-1` named as `charts/show` on its second
/// observation, not its first, and `plain` never named.
fn drawing_turn() -> InvocationRecord {
    turn(
        DRAWN,
        text(),
        vec![
            tool_call("call-1"),
            tool_call("plain"),
            tool_call("call-1").with_mcp_tool(mcp("charts", "show")),
        ],
    )
}
fn snapshot(invocations: Vec<InvocationRecord>) -> SessionSnapshot {
    SessionSnapshot {
        id: SessionId::new("session").unwrap(),
        provider: ProviderIdentity::new("provider", "model", "").unwrap(),
        provider_context: ProviderContext::Recorded(ExecutionSessionId::new("provider").unwrap()),
        invocations,
        queue_history: Vec::new(),
    }
}
/// The changes a record log keeps for `snapshot`, in its order.
fn record_log(snapshot: &SessionSnapshot) -> Vec<SessionChange> {
    let mut changes = vec![SessionChange::Opened {
        id: snapshot.id.clone(),
        provider: snapshot.provider.clone(),
        context: snapshot.provider_context.clone(),
    }];
    for record in &snapshot.invocations {
        let mut input = record.clone();
        input.events.clear();
        input.result = None;
        input.local_outcome = None;
        changes.push(SessionChange::InputAccepted(Box::new(input)));
        changes.extend(
            record
                .events
                .iter()
                .cloned()
                .map(SessionChange::ProviderObservation),
        );
        changes.push(SessionChange::LocalSettlement {
            execution_id: record.request.execution_id.clone(),
            before: None,
            after: record.result.clone().unwrap(),
            local_outcome: record.local_outcome,
        });
    }
    changes
}

/// The rule three ways: as admission asks it of `earlier`, as restoration
/// asks it of the snapshot `earlier` then the message, and as a replayed
/// record log does. Each must agree with `expected`.
fn judged(earlier: Vec<InvocationRecord>, message: UserMessage, expected: Result<(), UnknownApp>) {
    let admitted = validate_against(&message, |execution| {
        earlier
            .iter()
            .find(|record| &record.request.execution_id == execution)
    });
    assert_eq!(admitted, expected, "admission");
    let mut invocations = earlier;
    invocations.push(turn("turn-2", message, Vec::new()));
    let snapshot = snapshot(invocations);
    let corrupt = |result: Result<(), StorageError>| match (result, expected) {
        (Ok(()), Ok(())) => {}
        (Err(StorageError::Corrupt(message)), Err(refusal)) => {
            assert_eq!(message, refusal.to_string())
        }
        (other, _) => panic!("expected {expected:?}, got {other:?}"),
    };
    corrupt(validation::validate(&snapshot));
    corrupt(fold_changes(None, &record_log(&snapshot)).map(drop));
}

#[test]
fn a1_an_app_an_earlier_mcp_tool_call_drew_is_taken_as_writer_and_as_giver() {
    judged(vec![drawing_turn()], from(drawn()), Ok(()));
    judged(vec![drawing_turn()], carrying([drawn(), drawn()]), Ok(()));
    judged(
        vec![drawing_turn()],
        from(drawn()).with_app_model_context(Vec::new()).unwrap(),
        Ok(()),
    );
}

#[test]
fn a2_an_app_of_a_turn_the_session_has_no_record_of_is_refused() {
    let unknown = app("turn-0", "call-1", mcp("charts", "show"));
    judged(
        vec![drawing_turn()],
        from(unknown.clone()),
        Err(UnknownApp::NoMcpToolCall),
    );
    judged(
        Vec::new(),
        carrying([unknown]),
        Err(UnknownApp::NoMcpToolCall),
    );
}

#[test]
fn a3_an_app_of_a_tool_call_its_turn_never_made_is_refused() {
    let unmade = app(DRAWN, "call-2", mcp("charts", "show"));
    judged(
        vec![drawing_turn()],
        from(unmade.clone()),
        Err(UnknownApp::NoMcpToolCall),
    );
    judged(
        vec![drawing_turn()],
        carrying([unmade]),
        Err(UnknownApp::NoMcpToolCall),
    );
}

#[test]
fn a4_an_app_of_a_tool_call_that_named_no_mcp_server_is_refused() {
    let plain = app(DRAWN, "plain", mcp("charts", "show"));
    judged(
        vec![drawing_turn()],
        from(plain.clone()),
        Err(UnknownApp::NoMcpToolCall),
    );
    judged(
        vec![drawing_turn()],
        carrying([plain]),
        Err(UnknownApp::NoMcpToolCall),
    );
}

#[test]
fn a5_an_app_naming_another_server_or_tool_than_its_call_was_to_is_refused() {
    for other in [mcp("maps", "show"), mcp("charts", "hide")] {
        let forged = app(DRAWN, "call-1", other);
        judged(
            vec![drawing_turn()],
            from(forged.clone()),
            Err(UnknownApp::DifferentMcpTool),
        );
        judged(
            vec![drawing_turn()],
            carrying([forged]),
            Err(UnknownApp::DifferentMcpTool),
        );
    }
}

#[test]
fn a6_an_app_of_the_messages_own_turn_is_refused_though_that_turn_made_the_call() {
    // Admission: the turn is not yet among those saved.
    let own = app("turn-2", "call-1", mcp("charts", "show"));
    for message in [from(own.clone()), carrying([own.clone()])] {
        assert_eq!(
            validate_against(&message, |_| None),
            Err(UnknownApp::NoMcpToolCall)
        );
        // Restoration: the turn did make that call, after its message.
        let restored = snapshot(vec![
            drawing_turn(),
            turn(
                "turn-2",
                message,
                vec![tool_call("call-1").with_mcp_tool(mcp("charts", "show"))],
            ),
        ]);
        let refused = Err(StorageError::Corrupt(UnknownApp::NoMcpToolCall.to_string()));
        assert_eq!(validation::validate(&restored), refused);
        assert_eq!(
            fold_changes(None, &record_log(&restored)).map(drop),
            refused
        );
    }
}

#[test]
fn a7_a_recorded_writer_does_not_carry_a_context_no_call_drew() {
    for (stray, refusal) in [
        (
            app(DRAWN, "call-2", mcp("charts", "show")),
            UnknownApp::NoMcpToolCall,
        ),
        (
            app(DRAWN, "call-1", mcp("charts", "hide")),
            UnknownApp::DifferentMcpTool,
        ),
    ] {
        // Last of the contexts, behind a recorded writer and a recorded one.
        let message = from(drawn())
            .with_app_model_context(carrying([drawn(), stray]).app_model_context().to_vec())
            .unwrap();
        judged(vec![drawing_turn()], message, Err(refusal));
    }
}

#[test]
fn a8_a_restored_message_naming_a_later_turns_call_is_corrupt() {
    // The drawing turn is saved after the message that names it.
    let restored = snapshot(vec![
        turn("turn-0", from(drawn()), Vec::new()),
        drawing_turn(),
    ]);
    let refused = Err(StorageError::Corrupt(UnknownApp::NoMcpToolCall.to_string()));
    assert_eq!(validation::validate(&restored), refused);
    // And the same order replayed from a record log (A9).
    assert_eq!(
        fold_changes(None, &record_log(&restored)).map(drop),
        refused
    );
}

/// The steered message's own turn.
const STEERED: &str = "steered";

/// The record log of the completed turns `earlier`, then `target` running,
/// observing `before`, then `message` steered natively into it as
/// [`STEERED`] (its `target_event_offset` the count of `before`), then
/// `target` observing `after`.
fn steered_log(
    earlier: &[InvocationRecord],
    target: &str,
    message: UserMessage,
    before: Vec<ToolCallUpdate>,
    after: Vec<ToolCallUpdate>,
) -> Vec<SessionChange> {
    let target = ExecutionId::new(target).unwrap();
    let event = |kind, target: Option<&ExecutionId>, before, stage, cause, actor| {
        InvocationSchedulingEvent {
            kind,
            target: target.cloned(),
            before,
            stage,
            cause,
            actor,
        }
    };
    let actor = || Some(ActionContext::new("user", "test", "invoke").unwrap());
    let mut running = turn(target.as_str(), text(), Vec::new());
    running.events.clear();
    running.result = None;
    running.local_outcome = None;
    running.submission = SubmissionMode::Queued;
    running.scheduling = vec![event(
        InvocationKind::Queued,
        None,
        None,
        InvocationStage::Queued,
        SchedulingCause::Submitted,
        actor(),
    )];
    let mut steering = turn(STEERED, message, Vec::new());
    steering.events.clear();
    steering.result = None;
    steering.local_outcome = None;
    steering.submission = SubmissionMode::Steering;
    steering.target_event_offset = Some(before.len());
    steering.scheduling = vec![event(
        InvocationKind::Steering,
        Some(&target),
        None,
        InvocationStage::Queued,
        SchedulingCause::Submitted,
        actor(),
    )];
    let observed = |tools: Vec<ToolCallUpdate>| {
        tools.into_iter().map(|update| {
            SessionChange::ProviderObservation(ExecutionEvent::new(
                target.clone(),
                ExecutionUpdate::Tool(update),
            ))
        })
    };
    // Opened, then each earlier turn whole.
    let mut changes = record_log(&snapshot(earlier.to_vec()));
    changes.extend([
        SessionChange::InputAccepted(Box::new(running)),
        SessionChange::QueueDecision(QueueHistoryRecord {
            mutation: QueueMutation::Admitted {
                id: target.clone(),
                kind: InvocationKind::Queued,
            },
            actor: actor(),
            scheduling_length: Some(1),
        }),
        SessionChange::QueueDecision(QueueHistoryRecord {
            mutation: QueueMutation::Selected { id: target.clone() },
            actor: None,
            scheduling_length: Some(1),
        }),
        SessionChange::SchedulingTransition {
            execution_id: target.clone(),
            event: event(
                InvocationKind::Queued,
                None,
                Some(InvocationStage::Queued),
                InvocationStage::Running,
                SchedulingCause::Dispatched,
                None,
            ),
        },
    ]);
    changes.extend(observed(before));
    changes.push(SessionChange::InputAccepted(Box::new(steering)));
    changes.push(SessionChange::SchedulingTransition {
        execution_id: ExecutionId::new(STEERED).unwrap(),
        event: event(
            InvocationKind::Steering,
            Some(&target),
            Some(InvocationStage::Queued),
            InvocationStage::Injected,
            SchedulingCause::SteeringInjected,
            None,
        ),
    });
    changes.extend(observed(after));
    changes
}

/// The saved invocation `id` of `snapshot`.
fn invocation_mut<'a>(snapshot: &'a mut SessionSnapshot, id: &str) -> &'a mut InvocationRecord {
    snapshot
        .invocations
        .iter_mut()
        .find(|record| record.request.execution_id.as_str() == id)
        .unwrap()
}

/// A message steered into a running turn (A1b, A1c, A8): the rule as
/// admission asks it of `earlier` and the target's calls saved before the
/// message, as a replayed record log does, and as restoration asks it of a
/// snapshot that holds the target whole, with the message edited in after a
/// person's steered the same way.
fn judged_steered_after(
    earlier: Vec<InvocationRecord>,
    target: &str,
    message: UserMessage,
    before: Vec<ToolCallUpdate>,
    after: Vec<ToolCallUpdate>,
    expected: Result<(), UnknownApp>,
) {
    let saved = turn(target, text(), before.clone());
    assert_eq!(
        validate_against(&message, |execution| {
            earlier
                .iter()
                .chain([&saved])
                .find(|record| &record.request.execution_id == execution)
        }),
        expected,
        "admission"
    );
    let corrupt = |result: Result<(), StorageError>, side: &str| match (result, expected) {
        (Ok(()), Ok(())) => {}
        (Err(StorageError::Corrupt(message)), Err(refusal)) => {
            assert_eq!(message, refusal.to_string(), "{side}")
        }
        (other, _) => panic!("{side}: expected {expected:?}, got {other:?}"),
    };
    corrupt(
        fold_changes(
            None,
            &steered_log(
                &earlier,
                target,
                message.clone(),
                before.clone(),
                after.clone(),
            ),
        )
        .map(drop),
        "replay",
    );
    let mut restored =
        fold_changes(None, &steered_log(&earlier, target, text(), before, after)).unwrap();
    invocation_mut(&mut restored, STEERED).request.user_message = message;
    corrupt(validation::validate(&restored), "restoration");
}

/// [`judged_steered_after`] with no earlier turn, steered into [`DRAWN`].
fn judged_steered(
    message: UserMessage,
    before: Vec<ToolCallUpdate>,
    after: Vec<ToolCallUpdate>,
    expected: Result<(), UnknownApp>,
) {
    judged_steered_after(Vec::new(), DRAWN, message, before, after, expected);
}

#[test]
fn a1b_a_message_steered_into_a_running_turn_names_a_call_that_turn_observed_before_it() {
    let shown = || tool_call("call-1").with_mcp_tool(mcp("charts", "show"));
    for message in [from(drawn()), carrying([drawn()])] {
        judged_steered(message.clone(), vec![shown()], Vec::new(), Ok(()));
        // Observed again after the message: the first observation counts.
        judged_steered(message.clone(), vec![shown()], vec![shown()], Ok(()));
        judged_steered(
            message,
            vec![tool_call("call-1"), shown()],
            vec![tool_call("plain")],
            Ok(()),
        );
    }
    let hidden = app(DRAWN, "call-1", mcp("charts", "hide"));
    judged_steered(
        from(hidden),
        vec![shown()],
        Vec::new(),
        Err(UnknownApp::DifferentMcpTool),
    );
}

#[test]
fn a8_a_message_steered_into_a_running_turn_naming_a_call_observed_at_or_after_it_is_corrupt() {
    let shown = || tool_call("call-1").with_mcp_tool(mcp("charts", "show"));
    for message in [from(drawn()), carrying([drawn()])] {
        // Observed exactly at the offset: the first event after the message.
        judged_steered(
            message.clone(),
            Vec::new(),
            vec![shown()],
            Err(UnknownApp::NoMcpToolCall),
        );
        // Seen before the message only without its MCP identity.
        judged_steered(
            message.clone(),
            vec![tool_call("call-1"), tool_call("plain")],
            vec![tool_call("plain"), shown()],
            Err(UnknownApp::NoMcpToolCall),
        );
    }
}

#[test]
fn a1c_a_message_steered_into_a_running_turn_names_an_earlier_turns_call_past_its_offset() {
    // `turn-1` completed with `call-1` at its index 2; `turn-2` is running
    // with no events when the message is steered into it (offset 0).
    let earlier = turn(
        DRAWN,
        text(),
        vec![
            tool_call("plain"),
            tool_call("other"),
            tool_call("call-1").with_mcp_tool(mcp("charts", "show")),
        ],
    );
    for message in [from(drawn()), carrying([drawn()])] {
        judged_steered_after(
            vec![earlier.clone()],
            "turn-2",
            message,
            Vec::new(),
            vec![tool_call("plain")],
            Ok(()),
        );
    }
}

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

#[test]
fn a10_a_persons_message_carrying_no_context_looks_nothing_up() {
    assert_eq!(
        validate_app_sources(&text(), |_, _| unreachable!("nothing to look up")),
        Ok(())
    );
    judged(Vec::new(), text(), Ok(()));
}

#[test]
fn a9_a_replayed_tool_call_rolled_back_with_its_unit_draws_no_app() {
    use crate::application::agent_execution::sessions::records::continuation::Continuation;
    let drawing = drawing_turn();
    let log = record_log(&snapshot(vec![drawing.clone()]));
    // Opened and the drawing turn's input; then its observations, which a
    // unit that fails afterwards takes back with it.
    let (opening, observed) = log.split_at(2);
    let message = |id: &str| {
        let mut record = turn(id, from(drawn()), Vec::new());
        record.events.clear();
        record.result = None;
        record.local_outcome = None;
        SessionChange::InputAccepted(Box::new(record))
    };
    let mut continuation = Continuation::empty();
    continuation.apply_unit(opening).unwrap();
    let mut failing = observed[..observed.len() - 2].to_vec();
    failing.push(SessionChange::InputAccepted(Box::new({
        let mut again = drawing.clone();
        again.events.clear();
        again.result = None;
        again.local_outcome = None;
        again
    })));
    assert!(continuation.apply_unit(&failing).is_err());
    assert_eq!(
        continuation.apply_unit(&[message("turn-2")]),
        Err(StorageError::Corrupt(UnknownApp::NoMcpToolCall.to_string()))
    );
    // Applied for good, the same observations draw it.
    continuation
        .apply_unit(&observed[..observed.len() - 2])
        .unwrap();
    assert_eq!(continuation.apply_unit(&[message("turn-2")]), Ok(()));
}

/// An app is named by its call's MCP identity as the session observed it:
/// Claude's harness spells the listed `rows.get` as `rows_get`, and that is
/// the identity compared (A1, A5), not the server's listing.
#[test]
fn an_app_names_its_call_as_the_session_observed_it() {
    let renamed = || {
        turn(
            DRAWN,
            text(),
            vec![tool_call("call-1").with_mcp_tool(mcp("mcptest", "rows_get"))],
        )
    };
    let listed = app(DRAWN, "call-1", mcp("mcptest", "rows.get"));
    judged(
        vec![renamed()],
        from(listed.clone()),
        Err(UnknownApp::DifferentMcpTool),
    );
    judged(
        vec![renamed()],
        carrying([listed]),
        Err(UnknownApp::DifferentMcpTool),
    );
    let observed = app(DRAWN, "call-1", mcp("mcptest", "rows_get"));
    judged(vec![renamed()], from(observed.clone()), Ok(()));
    judged(vec![renamed()], carrying([observed]), Ok(()));
}

/// No durable history holds one call as two MCP tools, so the app rule has
/// nothing for admission and restoration to disagree on. A snapshot or a
/// record log keeping a call seen as `charts/show` and then as `charts/hide`
/// is refused by the tool call itself (`ExecutionError::DifferentMcpTool`),
/// whichever identity a later message names, before that message's app is
/// asked about. Admission's side, where the second observation is refused
/// live and saved as nothing, is `admission_keeps_a_calls_first_mcp_identity`.
#[test]
fn no_durable_history_holds_one_call_as_two_mcp_tools() {
    let twice = turn(
        DRAWN,
        text(),
        vec![
            tool_call("call-1").with_mcp_tool(mcp("charts", "show")),
            tool_call("call-1").with_mcp_tool(mcp("charts", "hide")),
        ],
    );
    for named in [drawn(), app(DRAWN, "call-1", mcp("charts", "hide"))] {
        let restored = snapshot(vec![twice.clone(), turn("turn-2", from(named), Vec::new())]);
        let different = |result: Result<(), StorageError>| {
            assert!(
                matches!(&result, Err(StorageError::Corrupt(message)) if message.contains("DifferentMcpTool")),
                "{result:?}"
            )
        };
        different(validation::validate(&restored));
        different(fold_changes(None, &record_log(&restored)).map(drop));
    }
}
