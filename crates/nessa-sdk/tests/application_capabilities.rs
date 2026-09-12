use nessa_sdk::{
    application::dto::{EffectiveCapabilitiesDto, ModalitiesDto, ModelMetadataDto},
    domain::{
        effective_capabilities::value_objects::{
            BindingRestrictions, CapabilityRequirement as R, EffectiveCapabilities, Modality as M,
        },
        model_metadata::{
            entities::ModelMetadata,
            value_objects::{Modalities, ModelFeatures, TokenLimits},
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
        output: text,
        tool_use: true,
        reasoning: true,
        max_context_window_tokens: 1000,
        max_output_tokens: 200,
        knowledge_cutoff: "2026-01".into(),
        documentation_url: "https://example.com".into(),
    })
    .unwrap();
    let text = Modalities::new(true, false, false).unwrap();
    let binding = BindingRestrictions::new(
        ModelFeatures::new(text, text, true, false),
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
    assert_eq!(dto.reasoning, snapshot.supports(R::Reasoning));
    assert_eq!(dto.tool_use, snapshot.supports(R::ToolUse));
    assert!(snapshot.validate(&[R::Input(M::Image)], 1, 1).is_err());
    dto.input.image = true;
    dto.context_window_tokens = 1000;
    assert!(!snapshot.supports(R::Input(M::Image)));
    assert_eq!(snapshot.limits().max_context_window(), 600);
    assert_ne!(dto, EffectiveCapabilitiesDto::from(&snapshot));
}
