//! One stored command and the view the phone renders from it.

use super::{Cause, Initiator, PhoneCommandId, PhoneIntent, PhoneOutcome, UnknownCause};

/// The durable command and what the phone currently shows for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhoneRecord {
    intent: PhoneIntent,
    departed: bool,
    outcome: PhoneOutcome,
    sends: u8,
    successor: Option<PhoneCommandId>,
}
impl PhoneRecord {
    /// A draft. `conversation.send` has not been asked.
    pub(crate) fn held(intent: PhoneIntent) -> Self {
        Self {
            intent,
            departed: false,
            outcome: PhoneOutcome::Draft,
            sends: 0,
            successor: None,
        }
    }
    /// Mark the draft departed and unknown until an answer arrives.
    ///
    /// Only a draft can depart (`departure_is_durable_before_the_gateway_is_asked`).
    pub(crate) fn depart(self) -> Option<Self> {
        if self.departed || self.outcome != PhoneOutcome::Draft {
            return None;
        }
        Some(Self {
            departed: true,
            outcome: PhoneOutcome::Unknown(UnknownCause::NotAdmitted),
            ..self
        })
    }
    /// The send this row stores.
    pub fn intent(&self) -> &PhoneIntent {
        &self.intent
    }
    /// Whether a send may have left the device.
    pub fn departed(&self) -> bool {
        self.departed
    }
    /// What the phone shows.
    pub fn outcome(&self) -> PhoneOutcome {
        self.outcome
    }
    /// How many `conversation.send` calls this foreground budget has made.
    pub fn sends(&self) -> u8 {
        self.sends
    }
    /// The new draft created for a `turn_running` refusal, when one was.
    pub fn successor(&self) -> Option<&PhoneCommandId> {
        self.successor.as_ref()
    }
    /// Replace the foreground send count. The intent and the outcome stay.
    pub(crate) fn with_sends(self, sends: u8) -> Self {
        Self { sends, ..self }
    }
    /// Replace the rendered outcome. The intent and the send count stay.
    pub(crate) fn with_outcome(self, outcome: PhoneOutcome) -> Self {
        Self { outcome, ..self }
    }
    /// Name the draft that now holds this command's text.
    pub(crate) fn with_successor(self, successor: PhoneCommandId) -> Self {
        Self {
            successor: Some(successor),
            ..self
        }
    }
    /// Drop a successor that was not committed, so a replay can mint again.
    pub(crate) fn clear_successor(self) -> Self {
        Self {
            successor: None,
            ..self
        }
    }
    /// The rendered view, including the typed text.
    pub fn view(&self) -> PhoneView {
        PhoneView {
            intent: self.intent.clone(),
            outcome: self.outcome,
            successor: self.successor.clone(),
        }
    }
}

/// What the phone renders for one command. The text is the text that was typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhoneView {
    intent: PhoneIntent,
    outcome: PhoneOutcome,
    successor: Option<PhoneCommandId>,
}
impl PhoneView {
    /// The saved send, including the typed text.
    pub fn intent(&self) -> &PhoneIntent {
        &self.intent
    }
    /// The rendered outcome.
    pub fn outcome(&self) -> PhoneOutcome {
        self.outcome
    }
    /// Whether the phone may say this was submitted.
    pub fn claims_submission(&self) -> bool {
        self.outcome.claims_submission()
    }
    /// The draft that now holds the text after `turn_running`.
    pub fn successor(&self) -> Option<&PhoneCommandId> {
        self.successor.as_ref()
    }
}

/// One audit row written in the same transaction as the outcome it records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntentEvent {
    /// Why the outcome changed.
    pub cause: Cause,
    /// The outcome after the change.
    pub outcome: PhoneOutcome,
    /// Who asked.
    pub initiator: Initiator,
    /// When the outbox observed the transition, on the injected clock.
    pub observed_at_ms: u64,
    /// The draft this return minted. Only [`Cause::ReturnedToDraft`] sets it.
    pub successor: Option<PhoneCommandId>,
}
