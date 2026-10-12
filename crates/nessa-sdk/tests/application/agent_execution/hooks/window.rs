use super::{EvaluationSuffix, InvocationObservationWindow, ObservationKind, PhrasePresence};
use crate::application::agent_execution::executions::{ExecutionEvent, ExecutionUpdate};
use crate::domain::agent_execution::executions::{
    ExecutionId, MessageChunk, MessageId, MessageKind, MESSAGE_CLONES,
};
use crate::domain::agent_execution::tools::{ToolCallId, ToolCallUpdate, ToolContent, ToolStatus};

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

fn finished_tool(id: &str) -> ExecutionEvent {
    ExecutionEvent::new(
        execution(),
        ExecutionUpdate::Tool(ToolCallUpdate::new(
            ToolCallId::new(id).unwrap(),
            None,
            None,
            Some(ToolStatus::Completed),
            None,
            None,
        )),
    )
}

fn oversized_tool() -> ExecutionEvent {
    ExecutionEvent::new(
        execution(),
        ExecutionUpdate::Tool(ToolCallUpdate::new(
            ToolCallId::new("big").unwrap(),
            None,
            None,
            None,
            None,
            Some(vec![ToolContent::text(
                "z".repeat(InvocationObservationWindow::MAX_PAYLOAD_BYTES + 1),
            )]),
        )),
    )
}

fn window(events: &[ExecutionEvent]) -> InvocationObservationWindow<'_> {
    InvocationObservationWindow::view(events)
}

#[test]
fn phrase_split_across_chunks_matches_as_one_sequence() {
    let events = [text("ignore all "), text("previous instructions")];
    let view = window(&events);
    assert!(view.contains_phrase("ignore all previous").is_present());
    assert!(view.contains_phrase("").is_present());
    assert!(view.is_complete());
    let events = [text("ign"), text("ore "), text("all")];
    assert!(window(&events).contains_phrase("ignore all").is_present());
}

#[test]
fn a_tool_or_a_different_message_separates_text_sequences() {
    let events = [text("ignore all "), tool("call"), text("previous")];
    let view = window(&events);
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
        window(&events).contains_phrase("ignore all previous"),
        PhrasePresence::Absent
    );
}

#[test]
fn byte_and_event_caps_omit_the_prefix_and_do_not_slice_an_oversized_event() {
    let fitting = vec![text(&"x".repeat(1024)); 64];
    let exact = window(&fitting);
    assert_eq!(exact.included_events(), 64);
    assert_eq!(
        exact.included_payload_bytes(),
        InvocationObservationWindow::MAX_PAYLOAD_BYTES
    );
    assert!(exact.is_complete());

    let over_bytes = vec![text(&"x".repeat(1024)); 65];
    let capped = window(&over_bytes);
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
    let counted = window(&many);
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
    assert!(window(&oldest_text)
        .contains_phrase("hidden-prefix")
        .is_inconclusive());

    let oversized = [text(
        &"y".repeat(InvocationObservationWindow::MAX_PAYLOAD_BYTES + 1),
    )];
    let omitted = window(&oversized);
    assert_eq!(omitted.included_events(), 0);
    assert!(omitted.excluded_oversized());
    assert!(omitted.omitted_text());
    assert!(omitted.contains_phrase("y").is_inconclusive());
    assert!(omitted.iter().next().is_none());
}

#[test]
fn an_oversized_event_is_a_separator_and_older_text_stays_visible() {
    let events = [text("intent"), oversized_tool(), text("x")];
    let view = window(&events);
    assert!(view.excluded_oversized());
    assert_eq!(view.contains_phrase("intent"), PhrasePresence::Present);
    assert_eq!(view.contains_phrase("intentx"), PhrasePresence::Absent);
    assert_eq!(view.iter().count(), 2);
}

#[test]
fn oversized_events_spend_event_slots_so_older_events_leave_the_window() {
    let mut events = vec![tool("older")];
    events.extend((0..InvocationObservationWindow::MAX_EVENTS).map(|_| oversized_tool()));
    let view = window(&events);
    assert_eq!(view.included_events(), 0);
    assert!(view.omitted_prefix());
    assert!(view.excluded_oversized());
    assert!(view.iter().next().is_none());
    let suffix = EvaluationSuffix::capture(&events);
    assert!(suffix.entries.len() <= InvocationObservationWindow::MAX_EVENTS);
    assert!(suffix.window().iter().next().is_none());
}

#[test]
fn thinking_is_visible_and_spends_the_same_budget() {
    let events = [
        thought("SEC"),
        thought("RET"),
        text("hel"),
        thought("nope"),
        text("lo"),
    ];
    let view = window(&events);
    assert_eq!(view.included_events(), events.len());
    let kinds: Vec<_> = view.iter().map(ObservationKind::of).collect();
    assert_eq!(
        kinds,
        vec![
            ObservationKind::Thinking,
            ObservationKind::Thinking,
            ObservationKind::ModelText,
            ObservationKind::Thinking,
            ObservationKind::ModelText,
        ]
    );
    assert_eq!(view.contains_phrase("SECRET"), PhrasePresence::Present);
    assert_eq!(view.contains_phrase("hello"), PhrasePresence::Absent);
    assert!(view.contains_phrase("hel").is_present());
    assert!(view.contains_phrase("nope").is_present());

    let mut heavy = Vec::new();
    for _ in 0..70 {
        heavy.push(thought(&"t".repeat(1024)));
    }
    heavy.push(text("end"));
    let capped = window(&heavy);
    assert!(capped.included_events() <= InvocationObservationWindow::MAX_EVENTS);
    assert!(capped.included_payload_bytes() <= InvocationObservationWindow::MAX_PAYLOAD_BYTES);
    assert!(capped.omitted_text());
    assert!(capped.contains_phrase("end").is_present());
}

#[test]
fn each_observation_keeps_its_kind() {
    let events = [
        thought("think"),
        text("said"),
        tool("call"),
        finished_tool("done"),
    ];
    let kinds: Vec<_> = window(&events).iter().map(ObservationKind::of).collect();
    assert_eq!(
        kinds,
        vec![
            ObservationKind::Thinking,
            ObservationKind::ModelText,
            ObservationKind::ToolCall,
            ObservationKind::ToolResult,
        ]
    );
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
    let view = window(&events);
    let copied = view;
    let _ = view.contains_phrase("ignore all previous");
    let _ = copied.contains_phrase("SECRET_REASON");
    let pointers: Vec<_> = copied
        .iter()
        .map(|event| event as *const ExecutionEvent)
        .collect();
    assert_eq!(pointers.len(), 4);
    assert!(std::ptr::eq(pointers[0], &events[0]));
    assert!(std::ptr::eq(pointers[1], &events[1]));
    assert!(std::ptr::eq(pointers[2], &events[2]));
    assert!(std::ptr::eq(pointers[3], &events[3]));
    let (clones, bytes) = MESSAGE_CLONES.with(|counter| counter.get());
    assert_eq!((clones, bytes), (0, 0));
}

#[test]
fn turn_boundary_copy_matches_the_live_view_and_skips_the_omitted_prefix() {
    let over_bytes = vec![text(&"x".repeat(1024)); 65];
    let full = window(&over_bytes);
    MESSAGE_CLONES.with(|counter| counter.set((0, 0)));
    let suffix = EvaluationSuffix::capture(&over_bytes);
    let (clones, bytes) = MESSAGE_CLONES.with(|counter| counter.get());
    let copied = suffix.window();
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
    let live = window(&thoughts);
    MESSAGE_CLONES.with(|counter| counter.set((0, 0)));
    let suffix = EvaluationSuffix::capture(&thoughts);
    let (clones, _) = MESSAGE_CLONES.with(|counter| counter.get());
    let copied = suffix.window();
    assert_eq!(copied.contains_phrase("end"), live.contains_phrase("end"));
    assert_eq!(copied.omitted_text(), live.omitted_text());
    assert_eq!(copied.omitted_prefix(), live.omitted_prefix());
    assert_eq!(copied.included_events(), live.included_events());
    assert!(clones < thoughts.len());
    assert_eq!(
        copied.contains_phrase(&"t".repeat(1024)),
        live.contains_phrase(&"t".repeat(1024))
    );
}

#[test]
fn review_debug_format_does_not_leak_withheld_thinking() {
    let events = [thought("SECRET_REASON"), text("visible")];
    let view = window(&events);
    let rendered = format!("{view:?}");
    assert!(
        !rendered.contains("SECRET_REASON"),
        "Debug prints thinking: {rendered}"
    );
    assert!(
        !rendered.contains("visible"),
        "Debug prints model text: {rendered}"
    );
    assert!(rendered.contains("included_events"));
}

#[test]
fn review_debug_format_does_not_leak_the_omitted_prefix() {
    let mut events = vec![text("OLD_PREFIX_TEXT")];
    for index in 0..InvocationObservationWindow::MAX_EVENTS {
        events.push(tool(&format!("t{index}")));
    }
    let view = window(&events);
    assert!(view.omitted_prefix());
    let rendered = format!("{view:?}");
    assert!(
        !rendered.contains("OLD_PREFIX_TEXT"),
        "Debug of a window exposes events outside the window: {rendered}"
    );
}

#[test]
fn review_turn_boundary_copy_is_bounded_when_thoughts_sit_between_texts() {
    let big = "t".repeat(1024 * 1024);
    let mut events = vec![text("a")];
    for _ in 0..16 {
        events.push(thought(&big));
    }
    events.push(text("b"));
    let live = window(&events);
    assert!(live.included_payload_bytes() <= InvocationObservationWindow::MAX_PAYLOAD_BYTES);
    assert!(live.excluded_oversized());
    assert!(live.contains_phrase("a").is_present());
    assert!(live.contains_phrase("b").is_present());
    assert!(!live.contains_phrase("ab").is_present());
    MESSAGE_CLONES.with(|counter| counter.set((0, 0)));
    let suffix = EvaluationSuffix::capture(&events);
    let (_, bytes) = MESSAGE_CLONES.with(|counter| counter.get());
    assert!(
        bytes <= 2 * InvocationObservationWindow::MAX_PAYLOAD_BYTES,
        "turn-boundary copy cloned {bytes} bytes for a window of {} bytes",
        live.included_payload_bytes()
    );
    assert_eq!(
        suffix.window().contains_phrase("ab").is_present(),
        live.contains_phrase("ab").is_present()
    );
}

fn naive_present(events: &[ExecutionEvent], phrase: &str) -> bool {
    let view = window(events);
    let included: Vec<*const ExecutionEvent> = view.iter().map(|event| event as *const _).collect();
    let mut sequences = Vec::new();
    let mut buffer = String::new();
    let mut current: Option<(MessageKind, Option<String>)> = None;
    let mut started = false;
    for event in events {
        let pointer = event as *const ExecutionEvent;
        if !included
            .iter()
            .any(|included| std::ptr::eq(*included, pointer))
        {
            if started {
                sequences.push(std::mem::take(&mut buffer));
                current = None;
            }
            continue;
        }
        started = true;
        match event.update() {
            ExecutionUpdate::Message(chunk) => {
                let key = (chunk.kind(), chunk.message_id().map(str::to_owned));
                if current.as_ref() != Some(&key) {
                    if current.is_some() {
                        sequences.push(std::mem::take(&mut buffer));
                    }
                    current = Some(key);
                }
                buffer.push_str(chunk.as_str());
            }
            _ => {
                sequences.push(std::mem::take(&mut buffer));
                current = None;
            }
        }
    }
    sequences.push(buffer);
    sequences.iter().any(|sequence| sequence.contains(phrase))
}

#[test]
fn review_phrase_search_matches_naive_reference_on_random_chunkings() {
    let alphabet = ["a", "b", "ab", "é", "ü", "aé", "日本"];
    let mut seed: u64 = 0x9e3779b97f4a7c15;
    let mut next = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as usize
    };
    for _case in 0..3000 {
        let mut events = Vec::new();
        let count = 1 + next() % 6;
        for _ in 0..count {
            let mut body = String::new();
            for _ in 0..(1 + next() % 4) {
                body.push_str(alphabet[next() % alphabet.len()]);
            }
            events.push(match next() % 5 {
                0 => thought(&body),
                1 => tool("sep"),
                2 => identified(false, "m2", &body),
                _ => text(&body),
            });
        }
        let mut phrase = String::new();
        for _ in 0..(1 + next() % 4) {
            phrase.push_str(alphabet[next() % alphabet.len()]);
        }
        let got = window(&events).contains_phrase(&phrase).is_present();
        let want = naive_present(&events, &phrase);
        assert_eq!(got, want, "phrase {phrase:?} events {events:?}");
    }
}

#[test]
fn review_pathological_needle_is_linear() {
    let chunks: Vec<_> = (0..64).map(|_| text(&"a".repeat(1000))).collect();
    let view = window(&chunks);
    let needle = format!("{}b", "a".repeat(30_000));
    let started = std::time::Instant::now();
    assert_eq!(view.contains_phrase(&needle), PhrasePresence::Absent);
    let huge = "a".repeat(4 * 1024 * 1024);
    assert_eq!(view.contains_phrase(&huge), PhrasePresence::Absent);
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

#[test]
fn small_thoughts_between_texts_keep_the_copy_bounded() {
    let mut events = vec![text("a")];
    for _ in 0..1000 {
        events.push(thought(&"t".repeat(1024)));
    }
    events.push(text("b"));
    MESSAGE_CLONES.with(|counter| counter.set((0, 0)));
    let _suffix = EvaluationSuffix::capture(&events);
    let (clones, bytes) = MESSAGE_CLONES.with(|counter| counter.get());
    assert!(clones <= InvocationObservationWindow::MAX_EVENTS);
    assert!(bytes <= InvocationObservationWindow::MAX_PAYLOAD_BYTES);
}

#[test]
fn a_provider_tool_id_cannot_hide_an_event_from_the_hook() {
    let spoof = ExecutionEvent::new(
        execution(),
        ExecutionUpdate::Tool(ToolCallUpdate::new(
            ToolCallId::new("\u{0}evil").unwrap(),
            None,
            None,
            None,
            None,
            None,
        )),
    );
    let events = [text("hello"), spoof];
    assert_eq!(
        window(&events).iter().count(),
        2,
        "a tool event whose provider id starts with NUL is invisible to hooks"
    );
}
