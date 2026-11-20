//! One trace per decision: a `decision` span on target
//! [`DECISION_TARGET`], opened when an ask is kept and closed when it
//! settles, so what people do with the cards can be measured (is the small
//! card enough, are recommendations trusted, is the auto-pick wait right).
//!
//! It carries ids, counts and indexes, never the question, a label or what
//! anyone wrote. A decision still waiting when the gateway stops has no span
//! after it starts again; the one that settles it then is opened at settling,
//! marked `decision.reopened`, with the same timings, which count from when
//! it was asked.
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{ConversationId, SettledAsk};
use nessa_protocol::conversation::view::{
    AskUndo, DecisionSettings, OwnerAsk, OwnerDecisionOutcome,
};
use std::collections::HashMap;
use std::sync::Mutex;
use tracing::field::Empty;
use tracing::Span;

/// The target decision spans are on, for a filter or an exporter to pick.
pub const DECISION_TARGET: &str = "nessa_conversation::decisions";

/// The most spans held open at once: a decision is waiting on a person, so
/// this many waiting at once is far past use; past it a span is opened only
/// when its decision settles.
const MOST_OPEN: usize = 1024;

/// The open `decision` spans, by ask.
#[derive(Default)]
pub(crate) struct DecisionTraces {
    open: Mutex<HashMap<String, (ConversationId, Span)>>,
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

fn span(conversation: &ConversationId, ask: &OwnerAsk) -> Span {
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
    tracing::info_span!(
        target: DECISION_TARGET,
        "decision",
        "decision.ask_id" = %ask.ask_id,
        "conversation.id" = %conversation,
        "agent.provider" = Empty,
        "decision.choice_count" = ask.choices.len(),
        "decision.recommended_index" = ask.recommended,
        "decision.has_details" = has_details,
        "decision.undo_classes" = format_args!("{undo}{}", if differs { ",differ" } else { "" }),
        "decision.auto_pick.allowed" = ask.auto_pick_allowed(),
        "decision.auto_pick.on" = Empty,
        "decision.auto_pick.after_s" = Empty,
        "decision.reopened" = Empty,
        "decision.outcome" = Empty,
        "decision.picked_index" = Empty,
        "decision.picked_recommended" = Empty,
        "decision.picked_first_shown" = Empty,
        "decision.with_note" = Empty,
        "decision.time_to_answer_ms" = Empty,
    )
}

impl DecisionTraces {
    /// `ask` was kept for its owner, whose settings were `settings` then.
    pub(crate) fn asked(
        &self,
        conversation: &ConversationId,
        agent: Option<AgentId>,
        ask: &OwnerAsk,
        settings: Option<DecisionSettings>,
    ) {
        let span = span(conversation, ask);
        if let Some(agent) = agent {
            span.record("agent.provider", agent.name());
        }
        if let Some(settings) = settings {
            span.record("decision.auto_pick.on", settings.auto_pick);
            span.record(
                "decision.auto_pick.after_s",
                settings.auto_pick_after_seconds,
            );
        }
        span.in_scope(|| tracing::info!(target: DECISION_TARGET, "asked"));
        if let Ok(mut open) = self.open.lock() {
            if open.len() < MOST_OPEN || open.contains_key(&ask.ask_id) {
                open.insert(ask.ask_id.clone(), (conversation.clone(), span));
            }
        }
    }

    /// Something happened to a waiting decision: `auto_pick_paused`,
    /// `auto_pick_resumed`.
    pub(crate) fn event(&self, ask_id: &str, event: &'static str) {
        let Ok(open) = self.open.lock() else { return };
        if let Some((_, span)) = open.get(ask_id) {
            span.in_scope(|| tracing::info!(target: DECISION_TARGET, event, "decision event"));
        }
    }

    /// `settled` stopped waiting in `conversation`; its span ends here.
    pub(crate) fn settled(&self, conversation: &ConversationId, settled: &SettledAsk) {
        let held = self
            .open
            .lock()
            .ok()
            .and_then(|mut open| open.remove(&settled.ask.ask_id))
            .map(|(_, span)| span);
        let span = held.unwrap_or_else(|| {
            let span = span(conversation, &settled.ask);
            span.record("decision.reopened", true);
            span
        });
        let ask = &settled.ask;
        span.record("decision.outcome", outcome_name(&settled.outcome));
        let at_ms = match &settled.outcome {
            OwnerDecisionOutcome::Decided {
                text,
                choice,
                at_ms,
                ..
            } => {
                if let Some(index) = choice {
                    span.record("decision.picked_index", index);
                    span.record("decision.picked_recommended", *index == ask.recommended);
                    span.record("decision.picked_first_shown", *index == 0);
                    let label = ask
                        .choices
                        .get(usize::from(*index))
                        .map_or("", |choice| choice.label.trim());
                    // Auto-pick's own words after the label are no note.
                    let auto = matches!(
                        settled.outcome,
                        OwnerDecisionOutcome::Decided { auto: true, .. }
                    );
                    span.record("decision.with_note", !auto && text.trim() != label);
                }
                *at_ms
            }
            OwnerDecisionOutcome::Replaced { at_ms, .. } => *at_ms,
        };
        span.record(
            "decision.time_to_answer_ms",
            at_ms.saturating_sub(ask.asked_at_ms),
        );
        span.in_scope(|| {
            tracing::info!(
                target: DECISION_TARGET,
                outcome = outcome_name(&settled.outcome),
                "settled"
            );
        });
    }

    /// `conversation` was deleted: its decisions will never settle, so their
    /// spans end here, unsettled, and stop holding a place.
    pub(crate) fn erased(&self, conversation: &ConversationId) {
        if let Ok(mut open) = self.open.lock() {
            open.retain(|_, (of, _)| of != conversation);
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
    use std::io::Write;
    use std::sync::Arc;
    use tracing_subscriber::fmt::format::FmtSpan;

    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);
    impl Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
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
            summary: None,
            auto_pick: Some(AutoPick::Allowed),
        }
    }

    /// A decision's span opens when it is asked and closes when it settles,
    /// saying how and how fast, with no words from the card in it; one asked
    /// before a restart is opened at settling and marked so.
    #[test]
    fn a_decision_is_one_span_from_ask_to_settling_without_its_words() {
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .with_span_events(FmtSpan::CLOSE)
            .with_env_filter(format!("{DECISION_TARGET}=info"))
            .finish();
        let conversation = ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap();
        let traces = DecisionTraces::default();
        let first = ask("0123456789abcdef0123456789abcdef");
        tracing::subscriber::with_default(subscriber, || {
            traces.asked(
                &conversation,
                Some(AgentId::Claude),
                &first,
                Some(DecisionSettings::default()),
            );
            assert_eq!(traces.open_count(), 1);
            traces.event(&first.ask_id, "auto_pick_paused");
            traces.settled(
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
            traces.asked(
                &conversation,
                None,
                &ask("00112233445566778899aabbccddeeff"),
                None,
            );
            assert_eq!(traces.open_count(), 1);
            traces.erased(&conversation);
            assert_eq!(traces.open_count(), 0);
            // Asked before a restart: nothing open, so it is opened now.
            traces.settled(
                &conversation,
                &SettledAsk {
                    ask: ask("fedcba9876543210fedcba9876543210"),
                    outcome: OwnerDecisionOutcome::Decided {
                        text: "Keep".into(),
                        choice: Some(1),
                        at_ms: 4_000,
                        auto: false,
                    },
                },
            );
        });
        let text = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        let closes: Vec<&str> = text.lines().filter(|line| line.contains("close")).collect();
        // The erased one closes unsettled, between the two.
        assert_eq!(closes.len(), 3, "{text}");
        assert!(!closes[1].contains("decision.outcome"), "{}", closes[1]);
        for wanted in [
            "decision.outcome=\"auto_picked\"",
            "decision.picked_index=0",
            "decision.picked_recommended=true",
            "decision.with_note=false",
            "decision.time_to_answer_ms=300000",
            "decision.auto_pick.allowed=true",
            "decision.auto_pick.on=true",
            "decision.auto_pick.after_s=300",
            "agent.provider=\"claude\"",
            "decision.undo_classes=easy",
        ] {
            assert!(closes[0].contains(wanted), "{wanted} in {}", closes[0]);
        }
        assert!(text.contains("auto_pick_paused"), "{text}");
        for wanted in [
            "decision.reopened=true",
            "decision.outcome=\"picked\"",
            "decision.picked_recommended=false",
            "decision.with_note=false",
            "decision.time_to_answer_ms=3000",
        ] {
            assert!(closes[2].contains(wanted), "{wanted} in {}", closes[2]);
        }
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
}
