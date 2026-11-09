//! One agreement rule for a turn's saved initiation and message sender.
use crate::domain::agent_execution::{
    executions::{Initiation, Initiator},
    prompts::{MessageSender, UserMessage},
};

pub(super) fn validate_message_author(
    initiation: &Initiation,
    message: &UserMessage,
) -> Result<(), &'static str> {
    match (initiation.initiator(), message.sender()) {
        (Initiator::Agent(source), MessageSender::Agent(sender)) if source == sender => Ok(()),
        (Initiator::Agent(_), _) | (_, MessageSender::Agent(_)) => {
            Err("agent message sender and initiation disagree")
        }
        _ => Ok(()),
    }
}
