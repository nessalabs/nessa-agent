//! What a phone send is allowed to show, and the gateway answers that become it.
//!
//! The five names are the phone send contract's. `Draft` is earlier: the text
//! is saved and `conversation.send` has not been asked.

use nessa_protocol::product_contract::generated::ConversationErrorCode;

/// Who asked for the transition. Recovery is the process reopening the store,
/// not a person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Initiator {
    /// The person holding the phone.
    Person,
    /// The outbox reading receipts after a restart. Not a person.
    Recovery,
}

/// Why the stored outcome changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// The typed text was saved.
    Held,
    /// The send was marked departed.
    Departed,
    /// `conversation.send` answered.
    SendAnswered,
    /// The send answer did not arrive.
    SendLost,
    /// The send answer arrived and was not a receipt or a known refusal.
    Unreadable,
    /// `conversation.receipt` answered.
    Lookup,
    /// The same bytes were sent again inside one foreground call.
    ForegroundSend,
    /// The person asked to retry after a lookup.
    ExplicitRetry,
    /// The foreground budget stopped another send.
    Bound,
    /// `turn_running` put the text on a new draft.
    ReturnedToDraft,
}

/// `conversation.send`'s disposition, as the contract names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// The turn is queued.
    Queued,
    /// The turn was injected into the running one.
    Injected,
    /// The turn has settled.
    Settled,
}

/// A receipt stage from `conversation.receipt`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiptStage {
    /// Saved, and the effect has not been authorized.
    Accepted,
    /// The effect was authorized and its success was not saved.
    Attempted,
    /// Creation's terminal stage. A submit lookup does not use it.
    Ready,
    /// Submit's terminal stage.
    Settled,
}

/// A settled receipt's effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiptEffect {
    /// The turn was enqueued.
    Dispatched,
    /// A queued turn was withdrawn. Not a submit effect.
    Withdrawn,
    /// An active turn was cancelled. Not a submit effect.
    Cancelled,
    /// The turn was already finished. Not a submit effect.
    AlreadyFinal,
    /// Refused before anything was enqueued.
    NoEffect,
    /// Saved, then refused before injection.
    RefusedAfterSave,
    /// Saved, then could not be checked before injection.
    UnavailableAfterSave,
}

/// A refusal the outbox can show on a failed send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhoneFailure {
    /// A conversation code, including `turn_running` and `drive_policy_unenforceable`.
    Conversation(ConversationErrorCode),
    /// The socket refused the credential. A lowered or revoked grant is answered
    /// here, before a conversation code.
    Forbidden,
    /// The socket did not accept the credential.
    Unauthorized,
}

/// Why an outcome is unknown. The effect may have happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnknownCause {
    /// Lookup found no receipt.
    NotAdmitted,
    /// The lookup did not finish, or an answer could not be read.
    LookupFailed,
    /// The receipt stage is `attempted`.
    Attempted,
    /// The send returned `submission_unresolved`.
    Unresolved,
}

/// What the phone shows for one saved command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhoneOutcome {
    /// Saved text. `conversation.send` has not been asked.
    Draft,
    /// The contract's `accepted`.
    Accepted,
    /// The contract's `running`.
    Running,
    /// The contract's `done`.
    Done,
    /// The contract's `failed`, with the code that said so.
    Failed(PhoneFailure),
    /// The contract's `unknown`.
    Unknown(UnknownCause),
}
impl PhoneOutcome {
    /// Whether the phone may say this command was submitted.
    ///
    /// Draft, failed, and unknown do not (`a_saved_draft_is_not_a_submission`).
    pub fn claims_submission(self) -> bool {
        matches!(self, Self::Accepted | Self::Running | Self::Done)
    }
}

/// What `conversation.send` answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendAnswer {
    /// A disposition receipt.
    Receipt(Delivery),
    /// A typed refusal, and no receipt.
    Refused(PhoneFailure),
    /// The call did not finish. The receipt must be read before another send.
    Lost,
    /// The call finished with bytes that are not a receipt or a known refusal.
    Broken,
}

/// What `conversation.receipt` answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookupAnswer {
    /// A saved receipt. `found` is true.
    Found {
        /// The saved stage.
        stage: ReceiptStage,
        /// Present once the stage is settled.
        effect: Option<ReceiptEffect>,
    },
    /// The principal has no receipt for this command. `found` is false.
    NotFound,
    /// A typed refusal. A deleted conversation is this, not [`Self::NotFound`].
    Refused(PhoneFailure),
    /// The same command id with a different fingerprint. `found` stays unset.
    Conflict,
    /// The lookup did not finish.
    Lost,
    /// The lookup finished with bytes that are not a receipt or a known refusal.
    Broken,
}
