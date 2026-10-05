//! What a configured model must take for a message ("The values, saved and
//! sent", B6 in `docs/design/mcp-app-calls.md`).
use super::*;
use crate::application::dto::{ImageInputLimitsDto, ModalitiesDto, ModelMetadataDto};
use crate::domain::{
    agent_execution::{
        prompts::{AppModelContext, ImageReference, McpAppSource, PromptText, UserMessage},
        tools::{McpTool, ToolCallId},
    },
    common::value_objects::{ImageMediaType, Sha256Digest},
    effective_capabilities::value_objects::BindingRestrictions,
    model_metadata::{
        entities::ModelMetadata,
        value_objects::{Modalities, ModelFeatures},
    },
};

/// A model that takes PNG images and no text.
fn images_only() -> EffectiveCapabilities {
    let model = ModelMetadata::try_from(ModelMetadataDto {
        provider: "anthropic".into(),
        model_id: "fixture".into(),
        display_name: "Fixture".into(),
        input: ModalitiesDto {
            text: false,
            image: true,
            audio: false,
        },
        image_input: Some(ImageInputLimitsDto {
            media_types: vec!["image/png".into()],
            max_encoded_bytes: 8,
            max_edge_px: 8000,
            many_images_max_edge_px: 2000,
            native_long_edge_px: 1568,
        }),
        output: ModalitiesDto {
            text: true,
            image: false,
            audio: false,
        },
        tool_use: true,
        reasoning: None,
        fast_mode: false,
        max_context_window_tokens: 1000,
        max_output_tokens: 100,
        knowledge_cutoff: "2026-01".into(),
        documentation_url: "https://example.com".into(),
    })
    .unwrap();
    EffectiveCapabilities::new(
        &model,
        BindingRestrictions::new(
            ModelFeatures::new(
                Modalities::new(false, true, false).unwrap(),
                Modalities::new(true, false, false).unwrap(),
                true,
                false,
                false,
            ),
            model.limits(),
        ),
        model.limits(),
    )
    .unwrap()
}
fn input(message: UserMessage) -> ExecutionRequest {
    ExecutionRequest {
        execution_id: ExecutionId::new("turn-2").unwrap(),
        user_message: message,
        estimated_input_tokens: 1,
        reserved_output_tokens: 1,
    }
}
fn image() -> UserMessage {
    UserMessage::new(
        None,
        vec![
            ImageReference::new(Sha256Digest::from_bytes([3; 32]), ImageMediaType::Png, 6).unwrap(),
        ],
        Vec::new(),
    )
    .unwrap()
}

#[test]
fn b6_a_message_carrying_app_contexts_needs_text_input_even_without_text() {
    let capabilities = images_only();
    let text = validate_configured_input(
        &capabilities,
        &input(UserMessage::text_only(PromptText::new("plot").unwrap())),
    );
    assert!(matches!(text, Err(AgentError::InvalidInput(_))), "{text:?}");
    let context = AppModelContext::new(
        McpAppSource::new(
            ExecutionId::new("turn-1").unwrap(),
            ToolCallId::new("call-1").unwrap(),
            McpTool::new("charts", "show").unwrap(),
        )
        .unwrap(),
        "update-1",
        Some("x".into()),
        None,
    )
    .unwrap()
    .unwrap();
    // The contexts reach the agent as a leading text block.
    assert_eq!(
        validate_configured_input(
            &capabilities,
            &input(image().with_app_model_context(vec![context]).unwrap())
        ),
        text
    );
    // The same images with no contexts are image input only.
    assert_eq!(
        validate_configured_input(&capabilities, &input(image())),
        Ok(())
    );
}
