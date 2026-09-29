use nessa_sdk::domain::common::value_objects::TokenLimits;
use nessa_sdk::{
    application::dto::{EffectiveCapabilitiesDto, ModalitiesDto, ModelMetadataDto, ReasoningDto},
    domain::{
        effective_capabilities::value_objects::{
            BindingRestrictions, CapabilityRequirement as R, EffectiveCapabilities, Modality as M,
        },
        model_metadata::{
            entities::ModelMetadata,
            value_objects::{Modalities, ModelFeatures},
        },
    },
};

#[test]
fn projection_describes_the_validating_snapshot_and_cannot_change_it() {
    let text = ModalitiesDto {
        text: true,
        image: false,
        audio: false,
    };
    let model = ModelMetadata::try_from(ModelMetadataDto {
        provider: "openai".into(),
        model_id: "fixture".into(),
        display_name: "Fixture".into(),
        input: ModalitiesDto {
            image: true,
            ..text
        },
        image_input: None,
        output: text,
        tool_use: true,
        reasoning: Some(ReasoningDto {
            effort_levels: vec!["low".into(), "high".into()],
        }),
        fast_mode: true,
        max_context_window_tokens: 1000,
        max_output_tokens: 200,
        knowledge_cutoff: "2026-01".into(),
        documentation_url: "https://example.com".into(),
    })
    .unwrap();
    let text = Modalities::new(true, false, false).unwrap();
    let binding = BindingRestrictions::new(
        ModelFeatures::new(text, text, true, false, false),
        TokenLimits::new(800, 150).unwrap(),
    );
    let snapshot =
        EffectiveCapabilities::new(&model, binding, TokenLimits::new(600, 100).unwrap()).unwrap();
    let mut dto = EffectiveCapabilitiesDto::from(&snapshot);
    assert_eq!(dto.provider, "openai");
    assert_eq!(dto.model_id, "fixture");
    assert_eq!(dto.context_window_tokens, 600);
    assert_eq!(dto.max_output_tokens, 100);
    assert_eq!(dto.input.image, snapshot.supports(R::Input(M::Image)));
    // The binding runs no reasoning, so the model's reasoning is not offered.
    assert_eq!(dto.reasoning, None);
    assert!(!snapshot.supports(R::Reasoning));
    assert_eq!(dto.fast_mode, snapshot.supports(R::FastMode));
    assert_eq!(dto.tool_use, snapshot.supports(R::ToolUse));
    assert!(snapshot.validate(&[R::Input(M::Image)], 1, 1).is_err());
    dto.input.image = true;
    dto.context_window_tokens = 1000;
    assert!(!snapshot.supports(R::Input(M::Image)));
    assert_eq!(snapshot.limits().max_context_window(), 600);
    assert_ne!(dto, EffectiveCapabilitiesDto::from(&snapshot));
}

#[test]
fn projection_carries_the_offered_levels_and_fast_mode() {
    let text = ModalitiesDto {
        text: true,
        image: false,
        audio: false,
    };
    let model = |reasoning| {
        ModelMetadata::try_from(ModelMetadataDto {
            provider: "anthropic".into(),
            model_id: "fixture".into(),
            display_name: "Fixture".into(),
            input: text,
            image_input: None,
            output: text,
            tool_use: true,
            reasoning,
            fast_mode: true,
            max_context_window_tokens: 1000,
            max_output_tokens: 200,
            knowledge_cutoff: "2026-01".into(),
            documentation_url: "https://example.com".into(),
        })
        .unwrap()
    };
    let text = Modalities::new(true, false, false).unwrap();
    let binding = BindingRestrictions::new(
        ModelFeatures::new(text, text, true, true, true),
        TokenLimits::new(800, 150).unwrap(),
    );
    let project = |model: &ModelMetadata| {
        EffectiveCapabilitiesDto::from(
            &EffectiveCapabilities::new(model, binding, TokenLimits::new(600, 100).unwrap())
                .unwrap(),
        )
    };
    let levels = Some(ReasoningDto {
        effort_levels: vec!["low".into(), "xhigh".into(), "max".into()],
    });
    let offered = project(&model(levels.clone()));
    assert_eq!(offered.reasoning, levels);
    assert!(offered.fast_mode);
    // Reasoning with no recorded level projects as reasoning with none.
    let unrecorded = Some(ReasoningDto {
        effort_levels: vec![],
    });
    assert_eq!(project(&model(unrecorded.clone())).reasoning, unrecorded);
    assert_eq!(project(&model(None)).reasoning, None);
}
