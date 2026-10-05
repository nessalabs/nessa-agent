//! The app a message names, row by row of "The app a message names"
//! (`docs/design/mcp-app-calls.md`): the rule as admission asks it of the
//! turns saved before the message, as restoration asks it of a snapshot, and
//! as a replayed record log asks it of each accepted input. Where a steered
//! message stands is `steering_position`'s, tested beside it.
use super::super::test_support::{
    app, drawn, from, invocation_mut, mcp, record_log, snapshot, steered_log, text, tool_call,
    turn, DRAWN, STEERED,
};
use super::*;
use crate::application::agent_execution::sessions::{
    records::fold_changes, validation, SessionChange, StorageError,
};
use crate::domain::agent_execution::tools::ToolCallUpdate;

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
