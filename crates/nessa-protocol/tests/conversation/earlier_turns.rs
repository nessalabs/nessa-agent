//! Rows for the turns a view leaves before its first one, read again a page
//! at a time (`earlier_turns`), and for how a view says they are there.

use super::tests::{completed_snapshot, projection};
use crate::conversation::projection::{bound_view, earlier_turns, Projection, MAX_VIEW_BYTES};
use crate::conversation::tool_uis::NoMcpToolUis;
use crate::conversation::view::{
    ConversationEarlier, ConversationMessage, ConversationPartPosition, ConversationPoint,
    ConversationView,
};
use nessa_sdk::application::agent_execution::executions::{ExecutionEvent, ExecutionUpdate};
use nessa_sdk::application::agent_execution::sessions::SessionSnapshot;
use nessa_sdk::domain::agent_execution::executions::{ExecutionId, MessageChunk};
use nessa_sdk::domain::agent_execution::prompts::{PromptText, UserMessage};
use std::sync::Arc;

/// A finished conversation of one turn per reply, the person saying `asked
/// <n>` before each.
fn chat(replies: &[String]) -> SessionSnapshot {
    let mut snapshot = completed_snapshot("turn-0", Vec::new());
    let template = snapshot.invocations[0].clone();
    snapshot.invocations.clear();
    for (index, reply) in replies.iter().enumerate() {
        let id = ExecutionId::new(format!("turn-{index}")).unwrap();
        let mut record = template.clone();
        record.request.execution_id = id.clone();
        record.request.user_message =
            UserMessage::text_only(PromptText::new(format!("asked {index}")).unwrap());
        record.events = vec![ExecutionEvent::new(
            id,
            ExecutionUpdate::Message(MessageChunk::text(reply.clone())),
        )];
        snapshot.invocations.push(record);
    }
    snapshot
}

fn view_of(snapshot: &SessionSnapshot) -> ConversationView {
    bound_view(
        Projection::new(
            "conversation".into(),
            projection().read().capabilities,
            Some(snapshot),
        )
        .read(),
    )
}

/// Where a reader reads before `message`: inside it when it starts partway.
fn before(message: &ConversationMessage) -> ConversationPoint {
    ConversationPoint {
        execution_id: message.execution_id.clone(),
        at: message.starts_at,
    }
}

fn page(
    snapshot: &SessionSnapshot,
    before: &ConversationPoint,
    after: Option<&ConversationPoint>,
) -> ConversationEarlier {
    earlier_turns(
        "conversation",
        snapshot,
        Arc::new(NoMcpToolUis),
        before,
        after,
        &Default::default(),
    )
    .unwrap()
}

/// What a reader holds once it has read back to the start: each turn's
/// person's message and reply, pieces of one turn joined at their seam.
fn read_back(snapshot: &SessionSnapshot) -> Vec<(String, String, String)> {
    let view = view_of(snapshot);
    let mut held: Vec<ConversationMessage> = view.messages.clone();
    let mut earlier = view.earlier;
    let mut pages = 0;
    while earlier {
        pages += 1;
        assert!(pages < 1_000, "reading back never reached the start");
        let page = page(snapshot, &before(&held[0]), None);
        assert!(!page.truncated);
        assert!(serde_json::to_vec(&page).unwrap().len() <= MAX_VIEW_BYTES);
        earlier = page.more;
        let mut joined = page.messages;
        for message in held {
            match joined.last_mut() {
                Some(last) if last.execution_id == message.execution_id => {
                    let at = message
                        .starts_at
                        .expect("a piece after the seam starts partway");
                    let mut parts = message.parts.into_iter();
                    if at.byte > 0 {
                        let first = parts.next().unwrap();
                        last.parts.last_mut().unwrap().text.push_str(&first.text);
                    }
                    last.parts.extend(parts);
                }
                _ => {
                    assert!(
                        joined.is_empty() || message.starts_at.is_none(),
                        "a piece starting partway follows the rest of its turn"
                    );
                    joined.push(message);
                }
            }
        }
        held = joined;
    }
    assert!(held[0].starts_at.is_none());
    held.into_iter()
        .map(|message| {
            let reply = message
                .parts
                .iter()
                .map(|part| part.text.as_str())
                .collect();
            (message.execution_id, message.user_text, reply)
        })
        .collect()
}

fn whole(replies: &[String]) -> Vec<(String, String, String)> {
    replies
        .iter()
        .enumerate()
        .map(|(index, reply)| {
            (
                format!("turn-{index}"),
                format!("asked {index}"),
                reply.clone(),
            )
        })
        .collect()
}

/// A view lists the newest turns and says older ones are earlier, not left
/// out: reading back before its first turn gives every turn, in order.
#[test]
fn a_long_chat_reads_back_to_its_first_turn() {
    let replies: Vec<String> = (0..60).map(|index| format!("reply {index}")).collect();
    let snapshot = chat(&replies);
    let view = view_of(&snapshot);
    assert!(view.earlier);
    assert!(!view.truncated);
    assert_eq!(view.messages[0].execution_id, "turn-36");
    assert_eq!(read_back(&snapshot), whole(&replies));
}

/// A turn too long for one frame is listed from partway, and the rest of it
/// — the person's message first — is read back in pieces that join exactly.
#[test]
fn a_turn_longer_than_a_frame_reads_back_whole() {
    let long: String = (0..30_000).map(|at| format!("w{at:05} ")).collect();
    let replies = vec!["first".to_owned(), long, "after".repeat(3)];
    let snapshot = chat(&replies);
    let mut snapshot_last = snapshot.clone();
    snapshot_last.invocations.truncate(2);
    let view = view_of(&snapshot_last);
    assert!(view.earlier && !view.truncated);
    assert_eq!(view.messages.len(), 1);
    assert!(view.messages[0].starts_at.is_some());
    assert!(view.messages[0].user_text.is_empty());
    assert_eq!(read_back(&snapshot_last), whole(&replies[..2]));
    assert_eq!(read_back(&snapshot), whole(&replies));
}

/// Text full of characters JSON escapes is cut by what it takes encoded, and
/// still joins back exactly.
#[test]
fn escaped_text_reads_back_whole() {
    let long = "\"\\\n\u{1}é".repeat(20_000);
    let replies = vec![long];
    assert_eq!(read_back(&chat(&replies)), whole(&replies));
}

/// After the view moved on, what lies between the oldest turn read and the
/// view's new first turn is read with `after`, and nothing past it.
#[test]
fn a_gap_is_read_between_two_points() {
    let replies: Vec<String> = (0..40).map(|index| format!("reply {index}")).collect();
    let snapshot = chat(&replies);
    let gap = page(
        &snapshot,
        &ConversationPoint {
            execution_id: "turn-30".into(),
            at: None,
        },
        Some(&ConversationPoint {
            execution_id: "turn-20".into(),
            at: None,
        }),
    );
    let ids: Vec<_> = gap
        .messages
        .iter()
        .map(|message| message.execution_id.as_str())
        .collect();
    assert_eq!(
        ids,
        (20..30)
            .map(|index| format!("turn-{index}"))
            .collect::<Vec<_>>()
    );
    assert!(!gap.more);
    // From partway through a turn, that turn starts there.
    let long: String = "z".repeat(1_000);
    let snapshot = chat(&[long.clone(), "next".into()]);
    let from = ConversationPartPosition { part: 0, byte: 400 };
    let gap = page(
        &snapshot,
        &ConversationPoint {
            execution_id: "turn-1".into(),
            at: None,
        },
        Some(&ConversationPoint {
            execution_id: "turn-0".into(),
            at: Some(from),
        }),
    );
    assert_eq!(gap.messages.len(), 1);
    assert_eq!(gap.messages[0].starts_at, Some(from));
    assert!(gap.messages[0].user_text.is_empty());
    assert_eq!(gap.messages[0].parts[0].text, long[400..]);
}

/// A page lists at most as many turns as the schema allows, however small.
#[test]
fn a_page_of_small_turns_stops_at_its_turn_limit() {
    let replies: Vec<String> = (0..400).map(|_| String::new()).collect();
    let snapshot = chat(&replies);
    let first = page(
        &snapshot,
        &ConversationPoint {
            execution_id: "turn-399".into(),
            at: None,
        },
        None,
    );
    assert_eq!(first.messages.len(), 128);
    assert!(first.more);
    assert_eq!(read_back(&snapshot), whole(&replies));
}

/// A point this history does not hold, or a place its turn does not have, is
/// no page at all: the reader's view came from another history.
#[test]
fn a_point_the_history_does_not_hold_reads_nothing() {
    let snapshot = chat(&["hello".into(), "there".into()]);
    let read = |before: ConversationPoint, after: Option<ConversationPoint>| {
        earlier_turns(
            "conversation",
            &snapshot,
            Arc::new(NoMcpToolUis),
            &before,
            after.as_ref(),
            &Default::default(),
        )
    };
    let at = |execution_id: &str, part, byte| ConversationPoint {
        execution_id: execution_id.into(),
        at: Some(ConversationPartPosition { part, byte }),
    };
    assert!(read(at("missing", 0, 0), None).is_none());
    assert!(read(at("turn-1", 2, 0), None).is_none());
    assert!(
        read(at("turn-1", 0, 5), None).is_none(),
        "past the text's end"
    );
    assert!(read(at("turn-0", 0, 0), Some(at("turn-1", 0, 0))).is_none());
    assert!(read(at("turn-1", 0, 2), None).is_some());
}

/// A fork's copied turns are the earliest of all: a view that starts after
/// the first turn leaves them for the page that reaches the start, which
/// lists them whole.
#[test]
fn a_forks_copied_turns_come_with_the_page_that_reaches_the_start() {
    use nessa_sdk::application::agent_execution::permissions::ActionContext;
    use nessa_sdk::application::agent_execution::sessions::{
        HandoffId, HandoffKind, HandoffRecord,
    };
    use nessa_sdk::domain::agent_execution::prompts::{CarriedHistory, CarriedTurn};
    use nessa_sdk::domain::agent_execution::sessions::SessionId;
    let replies: Vec<String> = (0..30).map(|index| format!("reply {index}")).collect();
    let mut snapshot = chat(&replies);
    let actor = ActionContext::new("person", "desktop", "fork").unwrap();
    snapshot.handoffs.push(HandoffRecord {
        id: HandoffId::of_request(&actor),
        kind: HandoffKind::Fork {
            source: SessionId::new("source").unwrap(),
            history: CarriedHistory::new(
                vec![CarriedTurn::new(Some("plot May"), Some("done")).unwrap()],
                0,
            )
            .unwrap(),
        },
        actor,
        at_ms: 1_700_000_000_000,
        after_execution: None,
    });
    let view = view_of(&snapshot);
    assert!(view.earlier);
    assert!(view.handoffs.iter().all(|handoff| handoff.turns.is_empty()));
    assert!(view
        .handoffs
        .iter()
        .all(|handoff| handoff.omitted_turns == 0));
    let first = page(&snapshot, &before(&view.messages[0]), None);
    assert!(!first.more);
    assert_eq!(first.messages[0].execution_id, "turn-0");
    assert_eq!(first.handoffs.len(), 1);
    assert_eq!(first.handoffs[0].turns.len(), 1);
}

/// A view with no turn has nothing to read before: copied turns too long for
/// it are counted as left out, never as earlier.
#[test]
fn a_fork_with_no_turn_after_it_counts_what_it_leaves_out() {
    use crate::conversation::projection::bound_view_within;
    use crate::conversation::view::{
        ConversationCarriedTurn, ConversationHandoff, ConversationHandoffKind,
    };
    let mut view = view_of(&chat(&[]));
    assert!(view.messages.is_empty());
    view.handoffs.push(ConversationHandoff {
        handoff_id: "fork".into(),
        kind: ConversationHandoffKind::Fork,
        at_ms: 0,
        after_execution_id: None,
        from: None,
        to: None,
        turns: (0..3)
            .map(|_| ConversationCarriedTurn {
                said: Some("s".repeat(1_000)),
                reply: Some("r".repeat(1_000)),
            })
            .collect(),
        omitted_turns: 1,
    });
    let view = bound_view_within(view, 2_000, true);
    assert!(!view.earlier);
    assert!(view.handoffs[0].turns.is_empty());
    assert_eq!(view.handoffs[0].omitted_turns, 4);
}

/// A queued turn is listed beside its pending entry until it runs: one that
/// gives way to fit the frame is said to be left out.
#[test]
fn a_queued_turn_that_gives_way_is_not_earlier() {
    use crate::conversation::projection::bound_view_within;
    use crate::conversation::view::{ConversationPending, ConversationPendingMode};
    let replies: Vec<String> = (0..3).map(|index| format!("reply {index}")).collect();
    let mut view = view_of(&chat(&replies));
    assert!(!view.earlier);
    view.pending.push(ConversationPending {
        execution_id: "turn-0".into(),
        text: "asked 0".into(),
        attachments: Vec::new(),
        files: Vec::new(),
        uploads: Vec::new(),
        app: None,
        agent: None,
        mode: ConversationPendingMode::Queued,
    });
    let size = serde_json::to_vec(&view).unwrap().len();
    let view = bound_view_within(view, size - 1, true);
    assert_ne!(view.messages[0].execution_id, "turn-0");
    assert!(view.truncated);
}

/// A person's message too long for a page of its own is left out, not
/// offered as earlier: a read before it could only give it back cut again,
/// and reading back would never get past it.
#[test]
fn a_message_too_long_for_any_page_is_left_out() {
    let mut snapshot = chat(&["first".to_owned(), "reply ".repeat(20_000)]);
    snapshot.invocations[1].request.user_message =
        UserMessage::text_only(PromptText::new("\u{1}".repeat(12_000)).unwrap());
    let view = view_of(&snapshot);
    assert!(view.truncated);
    assert_eq!(view.messages.len(), 1);
    assert!(view.messages[0].user_text.is_empty());
    let mut point = before(&view.messages[0]);
    let mut pages = 0;
    loop {
        pages += 1;
        assert!(pages < 100, "reading back never got past the message");
        let read = page(&snapshot, &point, None);
        let first = read.messages.first().unwrap();
        assert!(first.user_text.is_empty() || first.execution_id == "turn-0");
        if first.execution_id == "turn-0" {
            assert_eq!(first.user_text, "asked 0");
            assert!(!read.more);
            break;
        }
        assert!(read.more);
        point = before(first);
    }
}
