//! Product frames for one saved phone send. The outbox owns when they are asked.

use crate::outbox::domain::{
    Delivery, LookupAnswer, PhoneFailure, PhoneIntent, ReceiptEffect, ReceiptStage, SendAnswer,
};
use nessa_protocol::product::generated::{
    ConversationCommandOperation, ConversationCommandOutcome, ConversationCommandStage,
    ConversationDisposition, ConversationReceipt, ConversationReceiptParams,
    ConversationReceiptResult, ConversationSendParams, ImageAttachment,
};
/// `conversation.send` params from the saved intent. `files` is empty.
pub fn send_params(intent: &PhoneIntent) -> ConversationSendParams {
    ConversationSendParams {
        conversation_id: intent.conversation().to_string(),
        request_id: intent.command().as_str().to_owned(),
        execution_id: intent.turn().as_str().to_owned(),
        text: intent.text().to_owned(),
        attachments: attachments(intent),
        files: Vec::new(),
    }
}

/// `conversation.receipt` params. `operation` is `submit`. The bytes match the send.
pub fn receipt_params(intent: &PhoneIntent) -> ConversationReceiptParams {
    ConversationReceiptParams {
        conversation_id: intent.conversation().to_string(),
        request_id: intent.command().as_str().to_owned(),
        operation: ConversationCommandOperation::Submit,
        execution_id: Some(intent.turn().as_str().to_owned()),
        text: Some(intent.text().to_owned()),
        attachments: Some(attachments(intent)),
        files: Some(Vec::new()),
        agent: None,
        model: None,
        approval_mode: None,
        environment: None,
    }
}

/// A `conversation.send` payload, or a broken answer.
pub fn send_answer(receipt: ConversationReceipt) -> Result<SendAnswer, ()> {
    if receipt.execution_id.is_empty() {
        return Err(());
    }
    let delivery = match receipt.disposition {
        ConversationDisposition::Queued => Delivery::Queued,
        ConversationDisposition::Injected => Delivery::Injected,
        ConversationDisposition::Settled => Delivery::Settled,
    };
    Ok(SendAnswer::Receipt(delivery))
}

/// A `conversation.receipt` payload. A missing stage when `found` is true is broken.
pub fn lookup_answer(result: ConversationReceiptResult) -> Result<LookupAnswer, ()> {
    if !result.found {
        if result.stage.is_some() || result.outcome.is_some() {
            return Err(());
        }
        return Ok(LookupAnswer::NotFound);
    }
    let stage = match result.stage {
        Some(ConversationCommandStage::Accepted) => ReceiptStage::Accepted,
        Some(ConversationCommandStage::Attempted) => ReceiptStage::Attempted,
        Some(ConversationCommandStage::Ready) => ReceiptStage::Ready,
        Some(ConversationCommandStage::Settled) => ReceiptStage::Settled,
        None => return Err(()),
    };
    let effect = match result.outcome {
        None => None,
        Some(ConversationCommandOutcome::Dispatched) => Some(ReceiptEffect::Dispatched),
        Some(ConversationCommandOutcome::Withdrawn) => Some(ReceiptEffect::Withdrawn),
        Some(ConversationCommandOutcome::Cancelled) => Some(ReceiptEffect::Cancelled),
        Some(ConversationCommandOutcome::AlreadyFinal) => Some(ReceiptEffect::AlreadyFinal),
        Some(ConversationCommandOutcome::NoEffect) => Some(ReceiptEffect::NoEffect),
        Some(ConversationCommandOutcome::RefusedAfterSave) => Some(ReceiptEffect::RefusedAfterSave),
        Some(ConversationCommandOutcome::UnavailableAfterSave) => {
            Some(ReceiptEffect::UnavailableAfterSave)
        }
    };
    if stage != ReceiptStage::Settled && effect.is_some() {
        return Err(());
    }
    Ok(LookupAnswer::Found { stage, effect })
}

/// A refusal code this client was taught. Anything else is not guessed.
pub fn command_failure(code: &str) -> Result<PhoneFailure, ()> {
    match code {
        "forbidden" => Ok(PhoneFailure::Forbidden),
        "unauthorized" => Ok(PhoneFailure::Unauthorized),
        _ => serde_json::from_value(serde_json::Value::String(code.to_owned()))
            .map(PhoneFailure::Conversation)
            .map_err(|_| ()),
    }
}

#[cfg(test)]
#[path = "../../../tests/outbox/wire.rs"]
mod tests;

fn attachments(intent: &PhoneIntent) -> Vec<ImageAttachment> {
    intent
        .images()
        .iter()
        .map(|image| ImageAttachment {
            digest: image.digest(),
            mime_type: image.media_type().to_owned(),
            size: image.size(),
        })
        .collect()
}
