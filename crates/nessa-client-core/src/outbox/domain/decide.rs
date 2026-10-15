//! How a gateway answer moves a departed command. No store and no socket.

use super::{
    Delivery, LookupAnswer, PhoneFailure, PhoneOutcome, PhoneRecord, ReceiptEffect, ReceiptStage,
    SendAnswer, UnknownCause,
};
use nessa_protocol::product_contract::generated::ConversationErrorCode;

/// Sends one foreground call may make, the first included.
///
/// `foreground_send_stops_at_the_bound` counts them. A restart does not
/// continue the count into another send; it reads the receipt.
pub const FOREGROUND_SENDS: u8 = 3;

/// The next thing the outbox does with a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Save this record and stop.
    Done(PhoneRecord),
    /// Save this record, then read the receipt. `resend` is true only when the
    /// send answer did not arrive, so an unreadable answer is not sent again.
    Lookup { record: PhoneRecord, resend: bool },
    /// Save this record, then send the same bytes again.
    Send(PhoneRecord),
    /// Save this failed record and put its text on a new draft.
    ReturnToDraft(PhoneRecord),
}

/// Apply one `conversation.send` answer. A lost or unreadable answer looks
/// the receipt up; it does not by itself send again.
pub(crate) fn after_send(record: PhoneRecord, answer: SendAnswer) -> Step {
    let sends = record.sends().saturating_add(1);
    let record = record.with_sends(sends);
    match answer {
        SendAnswer::Receipt(Delivery::Queued) => {
            Step::Done(record.with_outcome(PhoneOutcome::Accepted))
        }
        SendAnswer::Receipt(Delivery::Injected) => {
            Step::Done(record.with_outcome(PhoneOutcome::Running))
        }
        SendAnswer::Receipt(Delivery::Settled) => {
            Step::Done(record.with_outcome(PhoneOutcome::Done))
        }
        SendAnswer::Refused(PhoneFailure::Conversation(
            ConversationErrorCode::SubmissionUnresolved,
        )) => Step::Done(record.with_outcome(PhoneOutcome::Unknown(UnknownCause::Unresolved))),
        SendAnswer::Refused(PhoneFailure::Conversation(ConversationErrorCode::TurnRunning)) => {
            Step::ReturnToDraft(record.with_outcome(PhoneOutcome::Failed(
                PhoneFailure::Conversation(ConversationErrorCode::TurnRunning),
            )))
        }
        SendAnswer::Refused(failure) => {
            Step::Done(record.with_outcome(PhoneOutcome::Failed(failure)))
        }
        SendAnswer::Lost => Step::Lookup {
            record,
            resend: true,
        },
        SendAnswer::Broken => Step::Lookup {
            record,
            resend: false,
        },
    }
}

/// Apply one `conversation.receipt` answer.
///
/// `allow_send` is true only inside the foreground call that already sent
/// and found nothing. A restart passes false, so an absent receipt stays
/// unknown (`recovery_does_not_send_when_the_receipt_is_absent`).
pub(crate) fn after_lookup(record: PhoneRecord, answer: LookupAnswer, allow_send: bool) -> Step {
    // An attempted receipt is already evidence of a submit. A restart that
    // cannot see a newer receipt leaves it attempted, so a later recovery
    // can still adopt a settled one. It is not downgraded to not-admitted.
    if !allow_send
        && matches!(
            record.outcome(),
            PhoneOutcome::Unknown(UnknownCause::Attempted)
        )
        && matches!(
            answer,
            LookupAnswer::NotFound | LookupAnswer::Lost | LookupAnswer::Broken
        )
    {
        return Step::Done(record);
    }
    match answer {
        LookupAnswer::Lost | LookupAnswer::Broken => {
            Step::Done(record.with_outcome(PhoneOutcome::Unknown(UnknownCause::LookupFailed)))
        }
        LookupAnswer::NotFound => {
            let record = record.with_outcome(PhoneOutcome::Unknown(UnknownCause::NotAdmitted));
            if allow_send && record.sends() < FOREGROUND_SENDS {
                Step::Send(record)
            } else {
                Step::Done(record)
            }
        }
        LookupAnswer::Conflict => Step::Done(record.with_outcome(PhoneOutcome::Failed(
            PhoneFailure::Conversation(ConversationErrorCode::SubmissionConflict),
        ))),
        LookupAnswer::Refused(PhoneFailure::Conversation(
            ConversationErrorCode::SubmissionUnresolved,
        )) => Step::Done(record.with_outcome(PhoneOutcome::Unknown(UnknownCause::Unresolved))),
        LookupAnswer::Refused(PhoneFailure::Conversation(ConversationErrorCode::TurnRunning)) => {
            Step::ReturnToDraft(record.with_outcome(PhoneOutcome::Failed(
                PhoneFailure::Conversation(ConversationErrorCode::TurnRunning),
            )))
        }
        LookupAnswer::Refused(failure) => {
            Step::Done(record.with_outcome(PhoneOutcome::Failed(failure)))
        }
        LookupAnswer::Found { stage, effect } => match submit_outcome(stage, effect) {
            Some(outcome) => Step::Done(record.with_outcome(outcome)),
            None => Step::Done(record),
        },
    }
}

/// The outcome a submit receipt stores.
///
/// `None` is not an outcome: stage `ready`, an effect on a stage that has not
/// settled, a settled receipt that omits its effect, or a settled stop effect
/// (`withdrawn`, `cancelled`, `already_final`). The row stays. `no_effect` is
/// failed (`a_settled_receipt_becomes_done_or_the_matching_refusal`).
pub(crate) fn submit_outcome(
    stage: ReceiptStage,
    effect: Option<ReceiptEffect>,
) -> Option<PhoneOutcome> {
    match stage {
        ReceiptStage::Accepted if effect.is_none() => Some(PhoneOutcome::Accepted),
        ReceiptStage::Attempted if effect.is_none() => {
            Some(PhoneOutcome::Unknown(UnknownCause::Attempted))
        }
        ReceiptStage::Settled => match effect {
            Some(ReceiptEffect::Dispatched) => Some(PhoneOutcome::Done),
            Some(ReceiptEffect::NoEffect) => Some(failed(ConversationErrorCode::Revoked)),
            Some(ReceiptEffect::RefusedAfterSave) => {
                Some(failed(ConversationErrorCode::RevokedAfterSave))
            }
            Some(ReceiptEffect::UnavailableAfterSave) => {
                Some(failed(ConversationErrorCode::UnavailableAfterSave))
            }
            _ => None,
        },
        ReceiptStage::Accepted | ReceiptStage::Attempted | ReceiptStage::Ready => None,
    }
}

fn failed(code: ConversationErrorCode) -> PhoneOutcome {
    PhoneOutcome::Failed(PhoneFailure::Conversation(code))
}
