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
        LookupAnswer::Found { stage, effect } => {
            Step::Done(record.with_outcome(from_receipt(stage, effect)))
        }
    }
}

fn from_receipt(stage: ReceiptStage, effect: Option<ReceiptEffect>) -> PhoneOutcome {
    match stage {
        ReceiptStage::Accepted => PhoneOutcome::Accepted,
        ReceiptStage::Attempted | ReceiptStage::Ready => {
            PhoneOutcome::Unknown(UnknownCause::Attempted)
        }
        ReceiptStage::Settled => match effect {
            Some(ReceiptEffect::Dispatched) => PhoneOutcome::Done,
            Some(ReceiptEffect::NoEffect) => failed(ConversationErrorCode::Revoked),
            Some(ReceiptEffect::RefusedAfterSave) => {
                failed(ConversationErrorCode::RevokedAfterSave)
            }
            Some(ReceiptEffect::UnavailableAfterSave) => {
                failed(ConversationErrorCode::UnavailableAfterSave)
            }
            Some(
                ReceiptEffect::Withdrawn | ReceiptEffect::Cancelled | ReceiptEffect::AlreadyFinal,
            )
            | None => PhoneOutcome::Unknown(UnknownCause::Attempted),
        },
    }
}

fn failed(code: ConversationErrorCode) -> PhoneOutcome {
    PhoneOutcome::Failed(PhoneFailure::Conversation(code))
}
