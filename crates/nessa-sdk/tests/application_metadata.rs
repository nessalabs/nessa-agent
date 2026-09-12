use nessa_sdk::domain::common::value_objects::Date;
use nessa_sdk::{
    application::{
        dto::{ModalitiesDto, ModelMetadataDto},
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
        output: ModalitiesDto {
            text: true,
            image: false,
            audio: false,
        },
        tool_use: true,
        reasoning: false,
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
        Err(nessa_sdk::domain::model_metadata::MetadataError::InvalidUrl(_))
    ));
}
