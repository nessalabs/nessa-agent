//! Product frames for one saved phone send.
use super::{command_failure, lookup_answer, receipt_params, send_answer, send_params};
use crate::outbox::{
    Delivery, LookupAnswer, PhoneCommandId, PhoneFailure, PhoneImage, PhoneIntent, ReceiptEffect,
    ReceiptStage, SendAnswer,
};
use nessa_protocol::product::generated::{
    ConversationCommandOperation, ConversationCommandOutcome, ConversationCommandStage,
    ConversationDisposition, ConversationReceipt, ConversationReceiptResult,
};
use nessa_protocol::product_contract::generated::ConversationErrorCode;
use nessa_sdk::domain::agent_execution::executions::ExecutionId;
use nessa_sdk::domain::common::value_objects::{ImageMediaType, Sha256Digest};

fn intent() -> PhoneIntent {
    let image = PhoneImage::new(
        Sha256Digest::parse(
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap(),
        ImageMediaType::parse("image/png").unwrap(),
        8,
    )
    .unwrap();
    PhoneIntent::new(
        "01234567-89ab-4cde-8f01-23456789abcd",
        PhoneCommandId::new("command-1").unwrap(),
        ExecutionId::new("turn-1").unwrap(),
        "hello",
        vec![image],
    )
    .unwrap()
}

#[test]
fn send_params_repeat_the_saved_fields_and_carry_no_files() {
    let params = send_params(&intent());
    assert_eq!(params.request_id, "command-1");
    assert_eq!(params.execution_id, "turn-1");
    assert_eq!(params.text, "hello");
    assert!(params.files.is_empty());
    assert_eq!(params.attachments.len(), 1);
    assert_eq!(params.attachments[0].mime_type, "image/png");
    assert_eq!(params.attachments[0].size, 8);
}

#[test]
fn receipt_params_are_a_submit_lookup_of_the_same_bytes() {
    let params = receipt_params(&intent());
    assert_eq!(params.operation, ConversationCommandOperation::Submit);
    assert_eq!(params.request_id, "command-1");
    assert_eq!(params.execution_id.as_deref(), Some("turn-1"));
    assert_eq!(params.text.as_deref(), Some("hello"));
    assert_eq!(params.files.as_ref().map(Vec::len), Some(0));
    assert_eq!(params.attachments.as_ref().map(Vec::len), Some(1));
    assert!(params.agent.is_none());
}

#[test]
fn a_disposition_is_the_contract_delivery_and_a_blank_execution_is_not() {
    let receipt = ConversationReceipt {
        execution_id: "turn-1".to_owned(),
        disposition: ConversationDisposition::Injected,
    };
    assert_eq!(
        send_answer(receipt).unwrap(),
        SendAnswer::Receipt(Delivery::Injected)
    );
    let blank = ConversationReceipt {
        execution_id: String::new(),
        disposition: ConversationDisposition::Settled,
    };
    assert!(send_answer(blank).is_err());
}

#[test]
fn a_receipt_result_keeps_found_stage_and_effect_together() {
    let missing = ConversationReceiptResult {
        found: false,
        request_id: "command-1".to_owned(),
        stage: None,
        outcome: None,
    };
    assert_eq!(lookup_answer(missing).unwrap(), LookupAnswer::NotFound);
    let settled = ConversationReceiptResult {
        found: true,
        request_id: "command-1".to_owned(),
        stage: Some(ConversationCommandStage::Settled),
        outcome: Some(ConversationCommandOutcome::UnavailableAfterSave),
    };
    assert_eq!(
        lookup_answer(settled).unwrap(),
        LookupAnswer::Found {
            stage: ReceiptStage::Settled,
            effect: Some(ReceiptEffect::UnavailableAfterSave),
        }
    );
    let staged = ConversationReceiptResult {
        found: false,
        request_id: "command-1".to_owned(),
        stage: Some(ConversationCommandStage::Accepted),
        outcome: None,
    };
    assert!(lookup_answer(staged).is_err());
    let early_effect = ConversationReceiptResult {
        found: true,
        request_id: "command-1".to_owned(),
        stage: Some(ConversationCommandStage::Accepted),
        outcome: Some(ConversationCommandOutcome::Dispatched),
    };
    assert!(lookup_answer(early_effect).is_err());
}

#[test]
fn an_unknown_refusal_code_is_not_guessed() {
    assert_eq!(
        command_failure("forbidden").unwrap(),
        PhoneFailure::Forbidden
    );
    assert_eq!(
        command_failure("turn_running").unwrap(),
        PhoneFailure::Conversation(ConversationErrorCode::TurnRunning)
    );
    assert!(command_failure("not_a_real_code").is_err());
}
