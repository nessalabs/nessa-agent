use nessa_sdk::domain::{common::value_objects::Date, model_metadata::MetadataError};
use nessa_sdk::{
    application::{
        dto::{ModalitiesDto, ModelMetadataDto, ReasoningDto},
        model_catalog::ModelCatalog,
    },
    domain::model_metadata::{aggregates::Catalog, entities::ModelMetadata},
};

fn dto() -> ModelMetadataDto {
    ModelMetadataDto {
        provider: "openai".into(),
        model_id: "test-model".into(),
        display_name: "Test model".into(),
        input: ModalitiesDto {
            text: true,
            image: true,
            audio: false,
        },
        image_input: None,
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
        documentation_url: "https://example.com/model".into(),
    }
}

#[test]
fn application_accepts_injected_domain_catalog_and_projects_all_metadata() {
    let input = dto();
    let entity = ModelMetadata::try_from(input.clone()).unwrap();
    let domain = Catalog::new(Date::new("2026-09-11".into()).unwrap(), vec![entity]).unwrap();
    let app = ModelCatalog::new(domain);
    assert_eq!(app.select("openai", "test-model").unwrap(), input);
    let mut projection = app.models();
    projection[0].max_output_tokens = 0;
    assert_eq!(
        app.select("openai", "test-model")
            .unwrap()
            .max_output_tokens,
        100
    );
}

#[test]
fn importing_typed_dtos_cannot_bypass_domain_invariants() {
    let mut input = dto();
    input.max_output_tokens = input.max_context_window_tokens + 1;
    assert!(ModelMetadata::try_from(input.clone()).is_err());
    assert!(ModelCatalog::from_metadata("2026-09-11".into(), vec![input]).is_err());
}

#[test]
fn typed_import_rejects_malformed_documentation_urls_at_the_domain_boundary() {
    let mut input = dto();
    input.documentation_url = "relative/model".into();
    assert!(matches!(
        ModelMetadata::try_from(input),
        Err(MetadataError::InvalidUrl(_))
    ));
}

fn reasoning(levels: &[&str]) -> Option<ReasoningDto> {
    Some(ReasoningDto {
        effort_levels: levels.iter().map(|level| (*level).into()).collect(),
    })
}

/// ADR 302: no reasoning, reasoning with no recorded levels, and reasoning with
/// its levels each come back as they went in, levels in the provider's order.
#[test]
fn reasoning_and_fast_mode_round_trip_through_the_domain() {
    for (reasoning, fast_mode) in [
        (None, false),
        (reasoning(&[]), false),
        (
            reasoning(&["none", "low", "medium", "high", "xhigh", "max"]),
            true,
        ),
        (reasoning(&["max", "low"]), false),
    ] {
        let mut input = dto();
        input.reasoning = reasoning;
        input.fast_mode = fast_mode;
        let model = ModelMetadata::try_from(input.clone()).unwrap();
        assert_eq!(model.features().reasoning(), input.reasoning.is_some());
        assert_eq!(model.features().fast_mode(), fast_mode);
        assert_eq!(ModelMetadataDto::from(&model), input);
    }
}

#[test]
fn typed_import_refuses_effort_levels_the_domain_would_not_hold() {
    for levels in [
        &["low", "low"][..],
        &["Low"][..],
        &[""][..],
        &["x high"][..],
    ] {
        let mut input = dto();
        input.reasoning = reasoning(levels);
        assert!(
            matches!(
                ModelMetadata::try_from(input),
                Err(MetadataError::Invalid {
                    field: "reasoning effort",
                    ..
                })
            ),
            "{levels:?}"
        );
    }
}
