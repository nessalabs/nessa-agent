//! One trace per decision: a `decision` span, opened when an ask is kept and
//! closed when it settles, so what people do with the cards can be measured
//! (is the small card enough, are recommendations trusted, is the auto-pick
//! wait right). Told as [`Telemetry`] events, so nothing is kept or sent while
//! telemetry is off.
//!
//! It carries ids, counts and indexes, never the question, a label or what
//! anyone wrote. A decision still waiting when the gateway stops has no open
//! span after it starts again; the one that settles it then is told whole at
//! settling, marked `decision.reopened`, timed from when it was asked.
//!
//! ```text
//! asked    ──▶ span start "decision" at asked_at_ms (shape of the card)
//! paused / resumed ──▶ point inside it
//! settled  ──▶ span end (outcome, pick, time to answer)
//!          └─ not open (restart, past MOST_OPEN) ──▶ one whole span, reopened
//! erased   ──▶ span end, no outcome
//! ```
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{ConversationId, SettledAsk};
use nessa_protocol::conversation::view::{
    AskUndo, DecisionSettings, OwnerAsk, OwnerDecisionOutcome,
};
use nessa_telemetry::{Event, SpanRef, Telemetry};
use std::collections::HashMap;
use std::sync::Mutex;

use crate::application::turn_telemetry::at;

/// The name of a decision's span.
const DECISION_SPAN: &str = "decision";

/// The most spans held open at once: a decision is waiting on a person, so
/// this many waiting at once is far past use; past it a span is told whole
/// when its decision settles.
const MOST_OPEN: usize = 1024;

/// The open `decision` spans: each waiting ask's conversation, by ask.
#[derive(Default)]
pub(crate) struct DecisionTraces {
    open: Mutex<HashMap<String, ConversationId>>,
}

/// What a decision ended as, the way the traces say it.
pub(crate) fn outcome_name(outcome: &OwnerDecisionOutcome) -> &'static str {
    match outcome {
        OwnerDecisionOutcome::Decided { auto: true, .. } => "auto_picked",
        OwnerDecisionOutcome::Decided {
            choice: Some(_), ..
        } => "picked",
        OwnerDecisionOutcome::Decided { .. } => "own_words",
        OwnerDecisionOutcome::Replaced { .. } => "replaced",
    }
}

/// The span of decision `ask_id`.
fn decision_span(ask_id: &str) -> SpanRef {
    SpanRef::new(format!("decision/{ask_id}"), "decision")
}

/// What the card looked like when it was asked.
fn asked_attributes(event: Event, conversation: &ConversationId, ask: &OwnerAsk) -> Event {
    let recommended = ask.choices.get(usize::from(ask.recommended));
    let undo = recommended
        .and_then(|choice| choice.undo)
        .map_or("unsaid", AskUndo::as_str);
    let differs = ask
        .choices
        .iter()
        .any(|choice| choice.undo != recommended.and_then(|choice| choice.undo));
    let has_details = ask.summary.is_some()
        || ask.choices.iter().any(|choice| {
            choice.undo.is_some()
                || choice.effort.is_some()
                || !choice.pros.is_empty()
                || !choice.cons.is_empty()
        });
    event
        .attr("decision.ask_id", ask.ask_id.clone())
        .attr("conversation.id", conversation.to_string())
        .attr("decision.choice_count", ask.choices.len())
        .attr("decision.recommended_index", u64::from(ask.recommended))
        .attr("decision.has_details", has_details)
        .attr(
            "decision.undo_classes",
            format!("{undo}{}", if differs { ",differ" } else { "" }),
        )
        .attr("decision.auto_pick.allowed", ask.auto_pick_allowed())
}

/// How `settled` ended.
fn settled_attributes(mut event: Event, settled: &SettledAsk) -> Event {
    let ask = &settled.ask;
    event = event.attr("decision.outcome", outcome_name(&settled.outcome));
    let at_ms = match &settled.outcome {
        OwnerDecisionOutcome::Decided {
            text,
            choice,
            at_ms,
            auto,
        } => {
            if let Some(index) = choice {
                let label = ask
                    .choices
                    .get(usize::from(*index))
                    .map_or("", |choice| choice.label.trim());
                event = event
                    .attr("decision.picked_index", u64::from(*index))
                    .attr("decision.picked_recommended", *index == ask.recommended)
                    .attr("decision.picked_first_shown", *index == 0)
                    // Auto-pick's own words after the label are no note.
                    .attr("decision.with_note", !auto && text.trim() != label);
            }
            *at_ms
        }
        OwnerDecisionOutcome::Replaced { at_ms, .. } => *at_ms,
    };
    event.attr(
        "decision.time_to_answer_ms",
        at_ms.saturating_sub(ask.asked_at_ms),
    )
}

/// When `settled` settled.
fn settled_at(settled: &SettledAsk) -> u64 {
    match &settled.outcome {
        OwnerDecisionOutcome::Decided { at_ms, .. }
        | OwnerDecisionOutcome::Replaced { at_ms, .. } => *at_ms,
    }
}

impl DecisionTraces {
    /// `ask` was kept for its owner, whose settings were `settings` then.
    pub(crate) fn asked(
        &self,
        telemetry: &Telemetry,
        conversation: &ConversationId,
        agent: Option<AgentId>,
        ask: &OwnerAsk,
        settings: Option<DecisionSettings>,
    ) {
        if !telemetry.is_on() {
            return;
        }
        let Ok(mut open) = self.open.lock() else {
            return;
        };
        if open.len() >= MOST_OPEN && !open.contains_key(&ask.ask_id) {
            return;
        }
        open.insert(ask.ask_id.clone(), conversation.clone());
        drop(open);
        let mut event = asked_attributes(
            Event::span_start(
                DECISION_SPAN,
                decision_span(&ask.ask_id),
                at(ask.asked_at_ms),
            ),
            conversation,
            ask,
        )
        .attr_some("agent.provider", agent.map(|agent| agent.name()));
        if let Some(settings) = settings {
            event = event
                .attr("decision.auto_pick.on", settings.auto_pick)
                .attr(
                    "decision.auto_pick.after_s",
                    u64::from(settings.auto_pick_after_seconds),
                );
        }
        telemetry.emit(event);
    }

    /// Something named `name` happened at `at_ms` to a waiting decision:
    /// `auto_pick_paused`, `auto_pick_resumed`.
    pub(crate) fn event(
        &self,
        telemetry: &Telemetry,
        ask_id: &str,
        name: &'static str,
        at_ms: u64,
    ) {
        let open = self.open.lock().is_ok_and(|open| open.contains_key(ask_id));
        if open {
            telemetry.emit(Event::point(name, at(at_ms)).within(decision_span(ask_id)));
        }
    }

    /// `settled` stopped waiting in `conversation`; its span ends here.
    pub(crate) fn settled(
        &self,
        telemetry: &Telemetry,
        conversation: &ConversationId,
        settled: &SettledAsk,
    ) {
        if !telemetry.is_on() {
            return;
        }
        let ask = &settled.ask;
        let held = self
            .open
            .lock()
            .ok()
            .and_then(|mut open| open.remove(&ask.ask_id))
            .is_some();
        let span = decision_span(&ask.ask_id);
        let ended = at(settled_at(settled));
        let event = if held {
            Event::span_end(DECISION_SPAN, span, ended)
        } else {
            asked_attributes(
                Event::span(DECISION_SPAN, span, at(ask.asked_at_ms), ended),
                conversation,
                ask,
            )
            .attr("decision.reopened", true)
        };
        telemetry.emit(settled_attributes(event, settled));
    }

    /// `conversation` was deleted at `at_ms`: its decisions will never
    /// settle, so their spans end here, unsettled, and stop holding a place.
    pub(crate) fn erased(&self, telemetry: &Telemetry, conversation: &ConversationId, at_ms: u64) {
        let ended: Vec<String> = match self.open.lock() {
            Ok(mut open) => {
                let ended = open
                    .iter()
                    .filter(|(_, of)| *of == conversation)
                    .map(|(ask, _)| ask.clone())
                    .collect::<Vec<_>>();
                for ask in &ended {
                    open.remove(ask);
                }
                ended
            }
            Err(_) => return,
        };
        for ask in ended {
            telemetry.emit(Event::span_end(
                DECISION_SPAN,
                decision_span(&ask),
                at(at_ms),
            ));
        }
    }

    #[cfg(test)]
    pub(crate) fn open_count(&self) -> usize {
        self.open.lock().map_or(0, |open| open.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nessa_protocol::conversation::view::{AutoPick, OwnerAskChoice};
    use nessa_telemetry::{Shape, Sink, Value};
    use std::sync::Arc;
    use std::time::Duration;

    #[derive(Clone, Default)]
    struct Recording(Arc<Mutex<Vec<Event>>>);
    impl Sink for Recording {
        fn emit(&self, event: &Event) {
            self.0.lock().unwrap().push(event.clone());
        }
    }

    fn attribute<'a>(event: &'a Event, key: &str) -> Option<&'a Value> {
        event
            .attributes()
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value)
    }

    fn ask(ask_id: &str) -> OwnerAsk {
        let choice = |label: &str| OwnerAskChoice {
            label: label.into(),
            consequence: "It happens.".into(),
            undo: Some(AskUndo::Easy),
            effort: None,
            pros: Vec::new(),
            cons: Vec::new(),
        };
        OwnerAsk {
            ask_id: ask_id.into(),
            question: "A private question?".into(),
            choices: vec![choice("Rename"), choice("Keep")],
            recommended: 0,
            reason: "A private reason.".into(),
            asked_at_ms: 1_000,
            access: None,
            summary: None,
            auto_pick: Some(AutoPick::Allowed),
        }
    }

    /// A decision's span opens when it is asked and closes when it settles,
    /// saying how and how fast, with no words from the card in it; one asked
    /// before a restart is told whole at settling and marked so.
    #[test]
    fn a_decision_is_one_span_from_ask_to_settling_without_its_words() {
        let recording = Recording::default();
        let telemetry = Telemetry::new(vec![Box::new(recording.clone())], 64).unwrap();
        let conversation = ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap();
        let traces = DecisionTraces::default();
        let first = ask("0123456789abcdef0123456789abcdef");
        traces.asked(
            &telemetry,
            &conversation,
            Some(AgentId::Claude),
            &first,
            Some(DecisionSettings::default()),
        );
        assert_eq!(traces.open_count(), 1);
        traces.event(&telemetry, &first.ask_id, "auto_pick_paused", 2_000);
        traces.settled(
            &telemetry,
            &conversation,
            &SettledAsk {
                ask: first.clone(),
                outcome: OwnerDecisionOutcome::Decided {
                    text: "Rename. Picked for you: no answer in 5 minutes.".into(),
                    choice: Some(0),
                    at_ms: 301_000,
                    auto: true,
                },
            },
        );
        assert_eq!(traces.open_count(), 0);
        // A deleted conversation's waiting decisions stop holding a place.
        let erased = ask("00112233445566778899aabbccddeeff");
        traces.asked(&telemetry, &conversation, None, &erased, None);
        assert_eq!(traces.open_count(), 1);
        traces.erased(&telemetry, &conversation, 9_000);
        assert_eq!(traces.open_count(), 0);
        // Asked before a restart: nothing open, so it is told whole now.
        let reopened = ask("fedcba9876543210fedcba9876543210");
        traces.settled(
            &telemetry,
            &conversation,
            &SettledAsk {
                ask: reopened.clone(),
                outcome: OwnerDecisionOutcome::Decided {
                    text: "Keep".into(),
                    choice: Some(1),
                    at_ms: 4_000,
                    auto: false,
                },
            },
        );
        assert!(telemetry.flush(Duration::from_secs(5)));
        let events = recording.0.lock().unwrap().clone();
        let shapes: Vec<&str> = events
            .iter()
            .map(|event| match event.shape() {
                Shape::SpanStart { .. } => "start",
                Shape::SpanEnd { .. } => "end",
                Shape::Span { .. } => "whole",
                Shape::Point { .. } => "point",
                Shape::Measure { .. } => "measure",
            })
            .collect();
        assert_eq!(shapes, ["start", "point", "end", "start", "end", "whole"]);

        let started = &events[0];
        assert_eq!(started.at(), at(1_000));
        assert_eq!(
            started.shape(),
            &Shape::SpanStart {
                span: decision_span(&first.ask_id),
                parent: None,
                links: Vec::new(),
            }
        );
        for (key, value) in [
            ("decision.auto_pick.allowed", Value::from(true)),
            ("decision.auto_pick.on", true.into()),
            ("decision.auto_pick.after_s", 300_u64.into()),
            ("agent.provider", "claude".into()),
            ("decision.undo_classes", "easy".to_owned().into()),
        ] {
            assert_eq!(attribute(started, key), Some(&value), "{key}");
        }
        assert_eq!(events[1].name(), "auto_pick_paused");
        let ended = &events[2];
        assert_eq!(ended.at(), at(301_000));
        for (key, value) in [
            ("decision.outcome", Value::from("auto_picked")),
            ("decision.picked_index", 0_u64.into()),
            ("decision.picked_recommended", true.into()),
            ("decision.with_note", false.into()),
            ("decision.time_to_answer_ms", 300_000_u64.into()),
        ] {
            assert_eq!(attribute(ended, key), Some(&value), "{key}");
        }
        // The erased one ends unsettled.
        assert_eq!(attribute(&events[4], "decision.outcome"), None);
        let whole = &events[5];
        let Shape::Span { started, .. } = whole.shape() else {
            panic!("told whole")
        };
        assert_eq!(*started, at(1_000));
        for (key, value) in [
            ("decision.reopened", Value::from(true)),
            ("decision.outcome", "picked".into()),
            ("decision.picked_recommended", false.into()),
            ("decision.with_note", false.into()),
            ("decision.time_to_answer_ms", 3_000_u64.into()),
            ("decision.ask_id", reopened.ask_id.clone().into()),
        ] {
            assert_eq!(attribute(whole, key), Some(&value), "{key}");
        }
        let text = format!("{events:?}");
        for private in ["private", "Rename", "Keep", "Picked for you"] {
            assert!(!text.contains(private), "{private} in {text}");
        }
        assert_eq!(
            outcome_name(&OwnerDecisionOutcome::Replaced {
                by: "x".into(),
                at_ms: 0
            }),
            "replaced"
        );
    }

    #[test]
    fn nothing_is_held_while_telemetry_is_off() {
        let traces = DecisionTraces::default();
        let conversation = ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap();
        traces.asked(&Telemetry::off(), &conversation, None, &ask("a"), None);
        assert_eq!(traces.open_count(), 0);
    }
}
