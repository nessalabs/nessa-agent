//! `conversation.send` and `conversation.receipt` on the pinned session.
//!
//! ```text
//! PhoneGateway::send --> product_command(conversation.send)
//! PhoneGateway::lookup --> product_command(conversation.receipt)
//! ```
//!
//! Arrows are calls. A typed refusal leaves the connection up. This adapter
//! does not decide who may send.

use super::session::{CommandExchange, Session};
use crate::outbox::{
    command_failure, lookup_answer, receipt_params, send_answer, send_params, LookupAnswer,
    PhoneGateway, PhoneIntent, SendAnswer,
};
use crate::read_only_sync::application::GatewayError;
use nessa_protocol::product::generated::{
    product_method, ConversationReceipt, ConversationReceiptResult,
};
use serde::Serialize;

impl PhoneGateway for Session {
    fn send(&mut self, intent: &PhoneIntent) -> SendAnswer {
        match self.exchange(product_method::CONVERSATION_SEND, &send_params(intent)) {
            Ok(value) => match serde_json::from_value::<ConversationReceipt>(value) {
                Ok(receipt) => match send_answer(intent, receipt) {
                    Ok(answer) => answer,
                    Err(()) => self.broken(),
                },
                Err(_) => self.broken(),
            },
            Err(CommandExchange::Refused(code)) => match command_failure(&code) {
                Ok(failure) => SendAnswer::Refused(failure),
                Err(()) => self.broken(),
            },
            Err(CommandExchange::Lost(_)) => SendAnswer::Lost,
        }
    }

    fn lookup(&mut self, intent: &PhoneIntent) -> LookupAnswer {
        match self.exchange(
            product_method::CONVERSATION_RECEIPT,
            &receipt_params(intent),
        ) {
            Ok(value) => match serde_json::from_value::<ConversationReceiptResult>(value) {
                Ok(result) => match lookup_answer(intent, result) {
                    Ok(answer) => answer,
                    Err(()) => {
                        self.fail(GatewayError::Protocol);
                        LookupAnswer::Broken
                    }
                },
                Err(_) => {
                    self.fail(GatewayError::Protocol);
                    LookupAnswer::Broken
                }
            },
            Err(CommandExchange::Refused(code)) => match command_failure(&code) {
                Ok(failure) => LookupAnswer::Refused(failure),
                Err(()) => {
                    self.fail(GatewayError::Protocol);
                    LookupAnswer::Broken
                }
            },
            Err(CommandExchange::Lost(_)) => LookupAnswer::Lost,
        }
    }
}

impl Session {
    fn exchange<T: Serialize>(
        &mut self,
        method: &str,
        params: &T,
    ) -> Result<serde_json::Value, CommandExchange> {
        if let Err(error) = self.begin() {
            return Err(CommandExchange::Lost(error));
        }
        let result = self.product_command(method, params);
        if self.finish().is_err() {
            return Err(CommandExchange::Lost(GatewayError::Transport));
        }
        result
    }

    fn broken(&mut self) -> SendAnswer {
        self.fail(GatewayError::Protocol);
        SendAnswer::Broken
    }
}
