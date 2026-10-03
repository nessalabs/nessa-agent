//! A request's own rules, asked at admission and of every restored record:
//! no app a message names was drawn by a tool call of its own turn.
use super::*;
use crate::domain::agent_execution::{
    prompts::{AppModelContext, McpAppSource, MessageSender, PromptText},
    tools::{McpTool, ToolCallId},
};

fn app(turn: &str) -> McpAppSource {
    McpAppSource::new(
        ExecutionId::new(turn).unwrap(),
        ToolCallId::new("call-1").unwrap(),
        McpTool::new("charts", "show").unwrap(),
    )
    .unwrap()
}

fn request(message: UserMessage) -> ExecutionRequest {
    ExecutionRequest {
        execution_id: ExecutionId::new("turn-2").unwrap(),
        user_message: message,
        estimated_input_tokens: 1,
        reserved_output_tokens: 1,
    }
}

#[test]
fn a_message_naming_an_app_of_its_own_turn_is_refused_and_one_of_an_earlier_is_not() {
    let text = || UserMessage::text_only(PromptText::new("hi").unwrap());
    let context = |turn: &str| {
        AppModelContext::new(app(turn), "update-1", Some("x".into()), None)
            .unwrap()
            .unwrap()
    };
    for own in [
        text().sent_by(MessageSender::App(app("turn-2"))),
        text()
            .with_app_model_context(vec![context("turn-2")])
            .unwrap(),
    ] {
        assert!(matches!(
            request(own).validate_message(),
            Err(AgentError::InvalidInput(_))
        ));
    }
    let earlier = text()
        .sent_by(MessageSender::App(app("turn-1")))
        .with_app_model_context(vec![context("turn-1")])
        .unwrap();
    assert_eq!(request(earlier).validate_message(), Ok(()));
    assert_eq!(request(text()).validate_message(), Ok(()));
}
