//! `GenAiTelemetry`: the SDK's GenAI spans told inside the turn's span, at
//! the wall time their binding's clock says, and without any content.
use super::*;
use nessa_sdk::application::agent_execution::traces::{
    ContentCapture, FinishReason, ModelChannel, ToolFact, ToolProgress, TraceRecorder, TurnIdentity,
};
use nessa_sdk::infrastructure::clock::ClockSleep;
use nessa_telemetry::{Shape, Sink, Value};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

#[derive(Default)]
struct Kept(Mutex<Vec<Event>>);
struct Recording(Arc<Kept>);
impl Sink for Recording {
    fn emit(&self, event: &Event) {
        self.0 .0.lock().unwrap().push(event.clone());
    }
}

/// A binding clock whose origin was one million milliseconds after the epoch.
struct Origin;
impl Clock for Origin {
    fn now(&self) -> ClockInstant {
        ClockInstant::from_origin(Duration::ZERO)
    }
    fn unix_ms_at(&self, at: ClockInstant) -> u64 {
        1_000_000 + at.since_origin().as_millis() as u64
    }
    fn sleep_until(&self, _: ClockInstant) -> ClockSleep {
        Box::pin(async {})
    }
}

const CONVERSATION: &str = "00000000-0000-4000-8000-000000000001";

/// One turn that calls a tool, recorded with content capture on, so a
/// forwarded span would carry the text if anything let it through.
fn told() -> Vec<Event> {
    let kept = Arc::new(Kept::default());
    let telemetry = Telemetry::new(vec![Box::new(Recording(kept.clone()))], 64).unwrap();
    let exporter = Arc::new(GenAiTelemetry::new(telemetry.clone(), Arc::new(Origin)));
    let mut recorder = TraceRecorder::new(exporter, ContentCapture::on());
    let ms = Duration::from_millis;
    recorder.begin_turn(
        ms(10),
        TurnIdentity::new(
            CONVERSATION,
            "turn-1",
            "anthropic",
            "claude",
            "sonnet",
            None,
        ),
        "the secret question",
    );
    recorder.model_output(
        ms(20),
        ModelChannel::Text,
        Some("msg-1"),
        "the secret answer",
    );
    recorder.tool(
        ms(30),
        ToolFact::new(
            "tool-1",
            "Read",
            "function",
            ToolProgress::Running,
            Some("secret.txt".into()),
            None,
        ),
    );
    recorder.tool(
        ms(40),
        ToolFact::new(
            "tool-1",
            "Read",
            "function",
            ToolProgress::Completed,
            None,
            Some("the secret file".into()),
        ),
    );
    recorder.finish(ms(50), FinishReason::Stop);
    drop(recorder);
    assert!(telemetry.flush(Duration::from_secs(5)));
    let told = kept.0.lock().unwrap().clone();
    told
}

fn text(event: &Event, key: &str) -> Option<String> {
    event
        .attributes()
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, value)| match value {
            Value::Text(text) => text.to_string(),
            other => format!("{other:?}"),
        })
}

fn operation(event: &Event) -> String {
    text(event, "gen_ai.operation.name").unwrap()
}

fn span(event: &Event) -> (&SpanRef, Option<&SpanRef>, SystemTime, SystemTime) {
    match event.shape() {
        Shape::Span {
            span,
            parent,
            started,
            ..
        } => (span, parent.as_ref(), *started, event.at()),
        other => panic!("not a whole span: {other:?}"),
    }
}

#[test]
fn a_turns_gen_ai_spans_are_told_inside_its_turn_span_at_their_wall_time() {
    let told = told();
    let find = |op: &str| told.iter().find(|event| operation(event) == op).unwrap();
    let agent = find("invoke_agent");
    let chat = find("chat");
    let tool = find("execute_tool");
    let turn = turn_span(CONVERSATION, "turn-1");

    let (agent_ref, agent_parent, started, ended) = span(agent);
    assert_eq!(agent_ref.trace(), turn.trace(), "in the turn's own trace");
    assert_eq!(
        agent_parent,
        Some(&turn),
        "the agent's span is a child of the turn"
    );
    assert_eq!(
        started,
        at(1_000_010),
        "measured from the binding clock's origin"
    );
    assert_eq!(ended, at(1_000_050));

    for child in [chat, tool] {
        let (child_ref, parent, _, _) = span(child);
        assert_eq!(child_ref.trace(), turn.trace());
        assert_eq!(
            parent,
            Some(agent_ref),
            "model and tool calls sit under the agent"
        );
    }
    assert_eq!(text(tool, "gen_ai.tool.name").as_deref(), Some("Read"));
    assert_eq!(text(agent, "nessa.execution.id").as_deref(), Some("turn-1"));
}

#[test]
fn no_content_is_told_even_when_the_store_captures_it() {
    for event in told() {
        for (key, value) in event.attributes().iter() {
            assert!(!CONTENT.contains(&key), "{key} forwarded");
            assert!(
                !format!("{value:?}").contains("secret"),
                "{key} carries content"
            );
        }
    }
}
