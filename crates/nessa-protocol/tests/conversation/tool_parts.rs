//! One tool call is one part of its turn, however many updates it has (#418).
//!
//! A harness reports one call as an announcement and then updates under the
//! same `toolCallId`: Codex as an announcement, a bare status, and a
//! completion; Claude as three to five frames. The desktop draws a card, and
//! for an app a mount, per part, so a part per update was a card per update.
//!
//! The recorded frames are the SDK's parser fixtures. Turning a frame into an
//! update is the SDK's (its wire tests show every frame of one call keeps that
//! call's id); what reaches the projection from a frame, and all this tests
//! needs, is that id and the status the frame states.
use super::tests::{committed_tool_view, completed_snapshot, event, projection};
use crate::conversation::projection::{bound_view, Projection};
use crate::conversation::view::ConversationView;
use nessa_sdk::application::agent_execution::executions::{ExecutionEvent, ExecutionUpdate};
use nessa_sdk::application::agent_execution::sessions::SessionSnapshot;
use nessa_sdk::domain::agent_execution::executions::{ExecutionId, MessageChunk};
use nessa_sdk::domain::agent_execution::tools::{
    ToolCallId, ToolCallUpdate, ToolContent, ToolStatus,
};
use serde_json::Value;

const CODEX: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../nessa-sdk/tests/infrastructure/codex_acp/tools/fixtures/mcp_live_frames.json"
));
const CLAUDE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../nessa-sdk/tests/infrastructure/claude_acp/tools/fixtures/mcp_live_frames.json"
));

fn status(frame: &Value) -> Option<ToolStatus> {
    frame.get("status").map(|status| match status.as_str() {
        Some("pending") => ToolStatus::Pending,
        Some("in_progress") => ToolStatus::Running,
        Some("completed") => ToolStatus::Completed,
        Some("failed") => ToolStatus::Failed,
        _ => panic!("recorded status {status}"),
    })
}

fn update(id: &str, status: Option<ToolStatus>) -> ExecutionEvent {
    event(ExecutionUpdate::Tool(ToolCallUpdate::new(
        ToolCallId::new(id).unwrap(),
        None,
        None,
        status,
        None,
        None,
    )))
}

fn text(chunk: &str) -> ExecutionEvent {
    event(ExecutionUpdate::Message(MessageChunk::text(chunk)))
}

/// The turn's tool parts, as `(tool id, offset)` in order.
fn tool_parts(view: &ConversationView, turn: usize) -> Vec<(String, usize)> {
    view.messages[turn]
        .parts
        .iter()
        .filter(|part| part.kind == "tool")
        .map(|part| (part.tool_id.clone(), part.offset))
        .collect()
}

/// Every recorded call of one harness, a sentence of text after each, through
/// the projection: one part per call, at its announcement, and the call's
/// entry at the status its last frame stated.
fn one_part_per_recorded_call(recorded: &str) {
    let recorded: Value = serde_json::from_str(recorded).unwrap();
    let mut events = Vec::new();
    let mut expected = Vec::new();
    let mut last_status = Vec::new();
    for frames in recorded["calls"].as_object().unwrap().values() {
        let frames = frames.as_array().unwrap();
        let id = frames[0]["toolCallId"].as_str().unwrap();
        assert!(frames.len() >= 3, "{id}: a call of one frame tests nothing");
        expected.push((id.to_owned(), events.len()));
        for frame in frames {
            assert_eq!(frame["toolCallId"], id);
            events.push(update(id, status(frame)));
        }
        last_status.push(frames.iter().rev().find_map(status).unwrap());
        events.push(text("Done. "));
    }
    let view = committed_tool_view(&events);
    assert_eq!(tool_parts(&view, 0), expected);
    assert_eq!(view.tools.len(), expected.len());
    for ((id, _), status) in expected.iter().zip(last_status) {
        let tool = view.tools.iter().find(|tool| &tool.tool_id == id).unwrap();
        let shown = match status {
            ToolStatus::Completed => "completed",
            ToolStatus::Failed => "failed",
            other => panic!("a recorded call ended {other:?}"),
        };
        assert_eq!(tool.status, shown, "{id}");
    }
    // The text between the calls is all still there, in its own parts.
    let texts = view.messages[0]
        .parts
        .iter()
        .filter(|part| part.kind == "text")
        .count();
    assert_eq!(texts, expected.len());
    assert!(!view.truncated);
}

#[test]
fn each_recorded_codex_mcp_call_is_one_tool_part() {
    one_part_per_recorded_call(CODEX);
}

#[test]
fn each_recorded_claude_call_is_one_tool_part() {
    one_part_per_recorded_call(CLAUDE);
}

/// Row: a later update with only a status changes the call's entry, not the
/// turn's parts. The other way: a status-only update for a call not seen yet
/// is that call's first update, and gives it its part.
#[test]
fn a_bare_status_update_moves_the_status_and_adds_no_part() {
    let announced = vec![
        update("call", Some(ToolStatus::Running)),
        update("call", Some(ToolStatus::Running)),
    ];
    let shown = committed_tool_view(&announced);
    assert_eq!(tool_parts(&shown, 0), vec![("call".into(), 0)]);
    assert_eq!(shown.tools[0].status, "running");

    let mut first_of_another = announced;
    first_of_another.push(update("other", Some(ToolStatus::Running)));
    let shown = committed_tool_view(&first_of_another);
    assert_eq!(
        tool_parts(&shown, 0),
        vec![("call".into(), 0), ("other".into(), 2)]
    );
}

/// Row: the completion merges into the call's one entry — its result and
/// status replace what was there, a field it leaves out stays — and adds no
/// part. The part stays at the announcement's offset, and the parts after it
/// keep theirs.
#[test]
fn a_completion_merges_into_the_one_entry_at_the_announcements_place() {
    let id = ToolCallId::new("call").unwrap();
    let events = vec![
        event(ExecutionUpdate::Tool(ToolCallUpdate::new(
            id.clone(),
            Some("mcp.mcptest.report_rows".into()),
            None,
            Some(ToolStatus::Running),
            None,
            None,
        ))),
        text("Looking. "),
        event(ExecutionUpdate::Tool(
            ToolCallUpdate::new(id, None, None, Some(ToolStatus::Completed), None, None)
                .with_content(vec![ToolContent::text("Two rows.")]),
        )),
        text("Found two."),
    ];
    let shown = committed_tool_view(&events);
    let parts: Vec<_> = shown.messages[0]
        .parts
        .iter()
        .map(|part| (part.kind.as_str(), part.offset))
        .collect();
    assert_eq!(parts, vec![("tool", 0), ("text", 1), ("text", 3)]);
    assert_eq!(shown.tools.len(), 1);
    assert_eq!(shown.tools[0].title, "mcp.mcptest.report_rows");
    assert_eq!(shown.tools[0].status, "completed");
    assert_eq!(shown.tools[0].details, "Two rows.");
}

/// Row: a call is named by its turn and its id together, so the same id in
/// two turns is two calls, a part in each. The other way: within one turn it
/// is one call (the rows above).
#[test]
fn one_tool_id_in_two_turns_is_a_part_in_each() {
    let in_turn = |turn: &str| {
        let execution = ExecutionId::new(turn).unwrap();
        [Some(ToolStatus::Running), Some(ToolStatus::Completed)]
            .into_iter()
            .map(|status| {
                ExecutionEvent::new(
                    execution.clone(),
                    ExecutionUpdate::Tool(ToolCallUpdate::new(
                        ToolCallId::new("call").unwrap(),
                        None,
                        None,
                        status,
                        None,
                        None,
                    )),
                )
            })
            .collect::<Vec<_>>()
    };
    let mut snapshot = completed_snapshot("first", in_turn("first"));
    let mut second = snapshot.invocations[0].clone();
    second.request.execution_id = ExecutionId::new("second").unwrap();
    second.events = in_turn("second");
    snapshot.invocations.push(second);
    let capabilities = projection().read().capabilities;
    let shown =
        bound_view(Projection::new("conversation".into(), capabilities, Some(&snapshot)).read());
    assert_eq!(shown.messages.len(), 2);
    for turn in 0..2 {
        assert_eq!(tool_parts(&shown, turn), vec![("call".into(), 0)]);
    }
    assert_eq!(shown.tools.len(), 2);
}

/// Row: the view keeps the most recent tool entries only. A call whose entry
/// was let go keeps its one part, and an update for it brings the entry back
/// without a second part.
#[test]
fn an_update_after_its_entry_was_let_go_adds_no_second_part() {
    let mut events: Vec<_> = (0..17)
        .map(|call| update(&format!("call-{call}"), Some(ToolStatus::Running)))
        .collect();
    let before = committed_tool_view(&events);
    assert!(before.tools.iter().all(|tool| tool.tool_id != "call-0"));
    events.push(update("call-0", Some(ToolStatus::Completed)));
    let shown = committed_tool_view(&events);
    let parts = tool_parts(&shown, 0);
    assert_eq!(parts.len(), 17);
    assert_eq!(parts.iter().filter(|(id, _)| id == "call-0").count(), 1);
    assert!(shown
        .tools
        .iter()
        .any(|tool| tool.tool_id == "call-0" && tool.status == "completed"));
}

/// Row: a turn holds at most 512 parts, and a full turn never frees one. A
/// call that arrives when it is full gets no part on any of its updates; its
/// entry is still kept, and the view says it left something out. The other
/// way: the calls before it each kept their one part.
#[test]
fn a_call_past_the_turns_part_bound_gets_no_part() {
    let mut events: Vec<_> = (0..512)
        .map(|call| update(&format!("call-{call}"), Some(ToolStatus::Completed)))
        .collect();
    let full = committed_tool_view(&events);
    let parts = tool_parts(&full, 0);
    assert_eq!(parts.len(), 512);
    assert!(parts
        .iter()
        .enumerate()
        .all(|(call, (id, offset))| { *id == format!("call-{call}") && *offset == call }));
    for status in [ToolStatus::Running, ToolStatus::Completed] {
        events.push(update("late", Some(status)));
    }
    let shown = committed_tool_view(&events);
    let parts = tool_parts(&shown, 0);
    assert_eq!(parts.len(), 512);
    assert!(parts.iter().all(|(id, _)| id != "late"));
    assert!(shown
        .tools
        .iter()
        .any(|tool| tool.tool_id == "late" && tool.status == "completed"));
    assert!(shown.truncated);
}

/// A turn's calls in `execution`, each a running update then a completion.
fn calls_in(execution: &str, calls: &[&str]) -> Vec<ExecutionEvent> {
    let execution = ExecutionId::new(execution).unwrap();
    calls
        .iter()
        .flat_map(|call| {
            [Some(ToolStatus::Running), Some(ToolStatus::Completed)].map(|s| (call, s))
        })
        .map(|(call, status)| {
            ExecutionEvent::new(
                execution.clone(),
                ExecutionUpdate::Tool(ToolCallUpdate::new(
                    ToolCallId::new(*call).unwrap(),
                    None,
                    None,
                    status,
                    None,
                    None,
                )),
            )
        })
        .collect()
}

fn restored(snapshot: &SessionSnapshot) -> ConversationView {
    let capabilities = projection().read().capabilities;
    bound_view(Projection::new("conversation".into(), capabilities, Some(snapshot)).read())
}

/// Row: a record replayed onto its turn starts the turn's parts again, so
/// its calls are first seen again and each gets its part back. The other
/// way: replayed once more, still one part per call.
#[test]
fn a_replayed_record_gives_its_calls_their_parts_again() {
    let mut snapshot = completed_snapshot("execution", calls_in("execution", &["call"]));
    let replay = snapshot.invocations[0].clone();
    snapshot.invocations.push(replay.clone());
    snapshot.invocations.push(replay);
    let shown = restored(&snapshot);
    assert_eq!(shown.messages.len(), 1);
    assert_eq!(tool_parts(&shown, 0), vec![("call".into(), 0)]);
}

/// Row: a turn whose message is let go takes its calls with it, so a later
/// update in that execution starts a new message where the call is first
/// seen and gets its part. Reached here by events naming another execution
/// than their record's, the one way a message is let go while events for it
/// can still follow.
#[test]
fn a_call_in_a_turn_let_go_gets_a_part_in_its_new_message() {
    let mut snapshot = completed_snapshot("turn-0", calls_in("turn-0", &["call"]));
    for turn in 1..24 {
        let mut record = snapshot.invocations[0].clone();
        record.request.execution_id = ExecutionId::new(format!("turn-{turn}")).unwrap();
        record.events = Vec::new();
        snapshot.invocations.push(record);
    }
    let last = snapshot.invocations.last_mut().unwrap();
    last.events = calls_in("extra", &["other"]);
    last.events.extend(calls_in("turn-0", &["call"]));
    let shown = restored(&snapshot);
    let turn = shown
        .messages
        .iter()
        .position(|message| message.execution_id == "turn-0")
        .unwrap();
    assert_eq!(tool_parts(&shown, turn), vec![("call".into(), 0)]);
}
