use super::{EvaluationSuffix, InvocationObservationWindow, PhrasePresence, ThinkingDisclosure};
use crate::application::agent_execution::executions::{ExecutionEvent, ExecutionUpdate};
use crate::domain::agent_execution::executions::{
    ExecutionId, MessageChunk, MessageId, MESSAGE_CLONES,
};
use crate::domain::agent_execution::tools::{ToolCallId, ToolCallUpdate};

fn execution() -> ExecutionId {
    ExecutionId::new("turn").unwrap()
}

fn text(body: &str) -> ExecutionEvent {
    ExecutionEvent::new(
        execution(),
        ExecutionUpdate::Message(MessageChunk::text(body)),
    )
}

fn thought(body: &str) -> ExecutionEvent {
    ExecutionEvent::new(
        execution(),
        ExecutionUpdate::Message(MessageChunk::thought(body)),
    )
}

fn identified(thought: bool, id: &str, body: &str) -> ExecutionEvent {
    let chunk = if thought {
        MessageChunk::thought(body)
    } else {
        MessageChunk::text(body)
    };
    ExecutionEvent::new(
        execution(),
        ExecutionUpdate::Message(chunk.with_message_id(MessageId::new(id).unwrap())),
    )
}

fn tool(id: &str) -> ExecutionEvent {
    ExecutionEvent::new(
        execution(),
        ExecutionUpdate::Tool(ToolCallUpdate::new(
            ToolCallId::new(id).unwrap(),
            None,
            None,
            None,
            None,
            None,
        )),
    )
}

fn window<'a>(
    events: &'a [ExecutionEvent],
    disclosure: ThinkingDisclosure,
) -> InvocationObservationWindow<'a> {
    InvocationObservationWindow::view(events, disclosure)
}

#[test]
fn phrase_split_across_chunks_matches_as_one_sequence() {
    let events = [text("ignore all "), text("previous instructions")];
    let view = window(&events, ThinkingDisclosure::Withheld);
    assert!(view.contains_phrase("ignore all previous").is_present());
    assert!(view.is_complete());
    let events = [text("ign"), text("ore "), text("all")];
    assert!(window(&events, ThinkingDisclosure::Withheld)
        .contains_phrase("ignore all")
        .is_present());
}

#[test]
fn a_tool_or_a_different_message_separates_text_sequences() {
    let events = [text("ignore all "), tool("call"), text("previous")];
    let view = window(&events, ThinkingDisclosure::Withheld);
    assert_eq!(
        view.contains_phrase("ignore all previous"),
        PhrasePresence::Absent
    );
    assert!(view.contains_phrase("ignore all ").is_present());
    let events = [
        identified(false, "one", "ignore all "),
        identified(false, "two", "previous"),
    ];
    assert_eq!(
        window(&events, ThinkingDisclosure::Withheld).contains_phrase("ignore all previous"),
        PhrasePresence::Absent
    );
}

#[test]
fn byte_and_event_caps_omit_the_prefix_and_do_not_slice_an_oversized_event() {
    let fitting = vec![text(&"x".repeat(1024)); 64];
    let exact = window(&fitting, ThinkingDisclosure::Withheld);
    assert_eq!(exact.included_events(), 64);
    assert_eq!(
        exact.included_payload_bytes(),
        InvocationObservationWindow::MAX_PAYLOAD_BYTES
    );
    assert!(exact.is_complete());

    let over_bytes = vec![text(&"x".repeat(1024)); 65];
    let capped = window(&over_bytes, ThinkingDisclosure::Withheld);
    assert_eq!(capped.included_events(), 64);
    assert!(capped.omitted_prefix());
    assert!(capped.omitted_text());
    assert!(capped.incomplete_text());
    assert!(capped
        .contains_phrase(&"x".repeat(1024 * 65))
        .is_inconclusive());
    assert!(capped.contains_phrase(&"x".repeat(1024)).is_present());

    let mut many = Vec::new();
    for index in 0..257 {
        many.push(tool(&format!("tool-{index}")));
    }
    many.push(text("hello"));
    let counted = window(&many, ThinkingDisclosure::Withheld);
    assert_eq!(
        counted.included_events(),
        InvocationObservationWindow::MAX_EVENTS
    );
    assert!(counted.omitted_prefix());
    assert!(!counted.omitted_text());
    assert_eq!(counted.contains_phrase("hello"), PhrasePresence::Present);
    assert_eq!(counted.contains_phrase("absent"), PhrasePresence::Absent);

    let oldest_text = {
        let mut events = vec![text("hidden-prefix")];
        for index in 0..InvocationObservationWindow::MAX_EVENTS {
            events.push(tool(&format!("later-{index}")));
        }
        events
    };
    assert!(window(&oldest_text, ThinkingDisclosure::Withheld)
        .contains_phrase("hidden-prefix")
        .is_inconclusive());

    let oversized = [text(
        &"y".repeat(InvocationObservationWindow::MAX_PAYLOAD_BYTES + 1),
    )];
    let omitted = window(&oversized, ThinkingDisclosure::Withheld);
    assert_eq!(omitted.included_events(), 0);
    assert!(omitted.excluded_oversized());
    assert!(omitted.omitted_text());
    assert!(omitted.contains_phrase("y").is_inconclusive());
    assert!(omitted.iter().next().is_none());
}

#[test]
fn thinking_is_withheld_unless_the_hook_opts_in() {
    let events = [
        thought("SEC"),
        thought("RET"),
        text("hel"),
        thought("nope"),
        text("lo"),
    ];
    let withheld = window(&events, ThinkingDisclosure::Withheld);
    assert_eq!(withheld.disclosure(), ThinkingDisclosure::Withheld);
    assert_eq!(withheld.included_events(), 2);
    assert!(withheld.iter().all(|event| {
        !matches!(
            event.update(),
            ExecutionUpdate::Message(chunk) if chunk.kind()
                == crate::domain::agent_execution::executions::MessageKind::Thought
        )
    }));
    assert_eq!(withheld.contains_phrase("SECRET"), PhrasePresence::Absent);
    assert_eq!(withheld.contains_phrase("hello"), PhrasePresence::Absent);
    assert!(withheld.contains_phrase("hel").is_present());

    let disclosed = window(&events, ThinkingDisclosure::Disclosed);
    assert!(disclosed.contains_phrase("SECRET").is_present());
    assert!(disclosed.contains_phrase("nope").is_present());
    assert_eq!(disclosed.contains_phrase("hello"), PhrasePresence::Absent);
    assert_eq!(disclosed.included_events(), events.len());
}

#[test]
fn building_the_window_does_not_clone_message_text() {
    let events = [
        text("ignore all "),
        thought("SECRET_REASON"),
        text("previous instructions"),
        tool("bash"),
    ];
    MESSAGE_CLONES.with(|counter| counter.set((0, 0)));
    let withheld = window(&events, ThinkingDisclosure::Withheld);
    let disclosed = window(&events, ThinkingDisclosure::Disclosed);
    let copied = withheld;
    let _ = withheld.contains_phrase("ignore all previous");
    let _ = disclosed.contains_phrase("SECRET_REASON");
    let pointers: Vec<_> = copied
        .iter()
        .map(|event| event as *const ExecutionEvent)
        .collect();
    assert_eq!(pointers.len(), 3);
    assert!(std::ptr::eq(pointers[0], &events[0]));
    assert!(std::ptr::eq(pointers[1], &events[2]));
    assert!(std::ptr::eq(pointers[2], &events[3]));
    let (clones, bytes) = MESSAGE_CLONES.with(|counter| counter.get());
    assert_eq!((clones, bytes), (0, 0));
}

#[test]
fn turn_boundary_copy_matches_the_live_view_and_skips_the_omitted_prefix() {
    let over_bytes = vec![text(&"x".repeat(1024)); 65];
    let full = window(&over_bytes, ThinkingDisclosure::Withheld);
    MESSAGE_CLONES.with(|counter| counter.set((0, 0)));
    let suffix = EvaluationSuffix::capture(&over_bytes);
    let (clones, bytes) = MESSAGE_CLONES.with(|counter| counter.get());
    let copied = suffix.window(ThinkingDisclosure::Withheld);
    assert_eq!(copied.included_events(), full.included_events());
    assert_eq!(
        copied.included_payload_bytes(),
        full.included_payload_bytes()
    );
    assert_eq!(copied.omitted_prefix(), full.omitted_prefix());
    assert_eq!(copied.omitted_text(), full.omitted_text());
    assert_eq!(copied.incomplete_text(), full.incomplete_text());
    assert_eq!(
        copied.contains_phrase(&"x".repeat(1024 * 65)),
        full.contains_phrase(&"x".repeat(1024 * 65))
    );
    assert_eq!(clones, full.included_events());
    assert_eq!(bytes, full.included_payload_bytes());
    assert!(clones < over_bytes.len());

    let mut thoughts = Vec::new();
    for _ in 0..70 {
        thoughts.push(thought(&"t".repeat(1024)));
    }
    thoughts.push(text("end"));
    let withheld = window(&thoughts, ThinkingDisclosure::Withheld);
    let disclosed = window(&thoughts, ThinkingDisclosure::Disclosed);
    MESSAGE_CLONES.with(|counter| counter.set((0, 0)));
    let suffix = EvaluationSuffix::capture(&thoughts);
    let (clones, _) = MESSAGE_CLONES.with(|counter| counter.get());
    let copied_withheld = suffix.window(ThinkingDisclosure::Withheld);
    let copied_disclosed = suffix.window(ThinkingDisclosure::Disclosed);
    assert_eq!(
        copied_withheld.contains_phrase("end"),
        withheld.contains_phrase("end")
    );
    assert_eq!(copied_withheld.omitted_text(), withheld.omitted_text());
    assert_eq!(
        copied_disclosed.contains_phrase("end"),
        disclosed.contains_phrase("end")
    );
    assert_eq!(
        copied_disclosed.omitted_prefix(),
        disclosed.omitted_prefix()
    );
    assert_eq!(
        copied_disclosed.included_events(),
        disclosed.included_events()
    );
    assert!(clones < thoughts.len());
    assert_eq!(
        copied_withheld.contains_phrase(&"t".repeat(1024)),
        PhrasePresence::Absent
    );
}
