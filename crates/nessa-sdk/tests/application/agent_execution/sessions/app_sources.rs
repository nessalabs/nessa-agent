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

/// The record log of `turn-1` running, observing `before`, then `message`
/// steered natively into it as `turn-2` (its `target_event_offset` the count
/// of `before`), then `turn-1` observing `after`.
fn steered_log(
    message: UserMessage,
    before: Vec<ToolCallUpdate>,
    after: Vec<ToolCallUpdate>,
) -> Vec<SessionChange> {
    let target = ExecutionId::new(DRAWN).unwrap();
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
    let mut running = turn(DRAWN, text(), Vec::new());
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
    let mut steering = turn("turn-2", message, Vec::new());
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
    let mut changes = vec![
        SessionChange::Opened {
            id: SessionId::new("session").unwrap(),
            provider: ProviderIdentity::new("provider", "model", "").unwrap(),
            context: ProviderContext::Recorded(ExecutionSessionId::new("provider").unwrap()),
        },
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
    ];
    changes.extend(observed(before));
    changes.push(SessionChange::InputAccepted(Box::new(steering)));
    changes.push(SessionChange::SchedulingTransition {
        execution_id: ExecutionId::new("turn-2").unwrap(),
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

/// A message steered into a running turn (A1b, A8): the rule as admission
/// asks it of the turn's calls saved before the message, as a replayed record
/// log does, and as restoration asks it of a snapshot that holds the turn
/// whole, with the message edited in after a person's steered the same way.
fn judged_steered(
    message: UserMessage,
    before: Vec<ToolCallUpdate>,
    after: Vec<ToolCallUpdate>,
    expected: Result<(), UnknownApp>,
) {
    let saved = turn(DRAWN, text(), before.clone());
    assert_eq!(
        validate_against(&message, |execution| {
            (execution.as_str() == DRAWN).then_some(&saved)
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
            &steered_log(message.clone(), before.clone(), after.clone()),
        )
        .map(drop),
        "replay",
    );
    let mut restored = fold_changes(None, &steered_log(text(), before, after)).unwrap();
    restored.invocations[1].request.user_message = message;
    corrupt(validation::validate(&restored), "restoration");
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
