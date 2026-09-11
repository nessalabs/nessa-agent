use nessa_sdk::domain::common::value_objects::{Date, Url};
use nessa_sdk::domain::model_metadata::{
    aggregates::Catalog,
    entities::ModelMetadata,
    value_objects::Modalities,
    value_objects::ModelDescription,
    value_objects::ModelFeatures,
    value_objects::TokenLimits,
    value_objects::{ModelKey, ModelProvider},
    MetadataError,
};

fn model(provider: ModelProvider, image: bool) -> ModelMetadata {
    ModelMetadata::new(
        ModelKey::new(provider, "test-model".into()).unwrap(),
        ModelDescription::new(
            "Test model".into(),
            Date::new("2026-01".into()).unwrap(),
            Url::new("https://example.com/model").unwrap(),
        )
        .unwrap(),
        ModelFeatures::new(
            Modalities::new(true, image, false).unwrap(),
            Modalities::new(true, false, false).unwrap(),
            true,
            false,
        ),
        TokenLimits::new(1000, 100).unwrap(),
    )
}
fn verified_on() -> Date {
    Date::new("2026-09-11".into()).unwrap()
}

#[test]
fn model_identity_is_provider_scoped_and_preserves_exact_ids() {
    assert_ne!(
        model(ModelProvider::OpenAi, true).key(),
        model(ModelProvider::Anthropic, true).key()
    );
    for invalid in ["", " leading", "has space", "line\n", "control\0"] {
        assert!(ModelProvider::try_from(invalid).is_err());
        assert!(ModelKey::new(ModelProvider::OpenAi, invalid.into()).is_err());
    }
    assert_ne!(
        ModelKey::new(ModelProvider::OpenAi, "Model".into()).unwrap(),
        ModelKey::new(ModelProvider::OpenAi, "model".into()).unwrap()
    );
}

#[test]
fn token_limits_cannot_be_zero_or_exceed_the_model_window() {
    for (context, output) in [(0, 0), (0, 1), (100, 0), (100, 101)] {
        assert!(TokenLimits::new(context, output).is_err());
    }
    let limits = TokenLimits::new(100, 100).unwrap();
    assert_eq!(limits.max_context_window(), 100);
    assert_eq!(limits.max_output(), 100);
}

#[test]
fn modality_sets_require_support_and_preserve_explicit_false_values() {
    assert!(Modalities::new(false, false, false).is_err());
    let audio = Modalities::new(false, false, true).unwrap();
    assert!(audio.audio());
    assert!(!audio.text());
    assert!(!audio.image());
    let features = ModelFeatures::new(audio, audio, false, false);
    assert!(!features.tool_use());
    assert!(!features.reasoning());
}

#[test]
fn catalog_owns_the_uniqueness_boundary_and_requires_complete_verification_date() {
    assert!(matches!(
        Catalog::new(verified_on(), vec![]),
        Err(MetadataError::EmptyCatalog)
    ));
    assert!(matches!(
        Catalog::new(
            verified_on(),
            vec![
                model(ModelProvider::OpenAi, true),
                model(ModelProvider::OpenAi, false)
            ]
        ),
        Err(MetadataError::Duplicate(_))
    ));
    assert!(Catalog::new(
        Date::new("2026-09".into()).unwrap(),
        vec![model(ModelProvider::OpenAi, true)]
    )
    .is_err());
    let catalog = Catalog::new(
        verified_on(),
        vec![
            model(ModelProvider::OpenAi, true),
            model(ModelProvider::Anthropic, false),
        ],
    )
    .unwrap();
    assert!(catalog
        .find(model(ModelProvider::OpenAi, false).key())
        .unwrap()
        .features()
        .input()
        .image());
    assert!(!catalog
        .find(model(ModelProvider::Anthropic, true).key())
        .unwrap()
        .features()
        .input()
        .image());
    assert!(catalog
        .find(&ModelKey::new(ModelProvider::OpenAi, "missing".into()).unwrap())
        .is_none());
}

#[test]
fn descriptions_require_a_name_source_and_valid_date_without_inventing_precision() {
    assert!(ModelDescription::new(
        " ".into(),
        verified_on(),
        Url::new("https://example.com").unwrap()
    )
    .is_err());
    assert!(ModelDescription::new(
        "Model".into(),
        verified_on(),
        Url::new("http://example.com").unwrap()
    )
    .is_err());
    for invalid in [
        "2026-02-29",
        "1900-02-29",
        "2026-13",
        "0000-01",
        "2026-04-31",
    ] {
        assert!(Date::new(invalid.into()).is_err());
    }
    for valid in ["2000-02-29", "2024-02-29", "2026-06", "2026-09-11"] {
        assert_eq!(Date::new(valid.into()).unwrap().as_str(), valid);
    }
}

#[test]
fn separate_catalogs_keep_independent_model_facts() {
    let first = Catalog::new(verified_on(), vec![model(ModelProvider::OpenAi, true)]).unwrap();
    let second = Catalog::new(verified_on(), vec![model(ModelProvider::OpenAi, false)]).unwrap();
    assert!(first.models()[0].features().input().image());
    assert!(!second.models()[0].features().input().image());
}

#[test]
fn context_usage_uses_the_instances_window() {
    let limits = TokenLimits::new(200_000, 128_000).unwrap();
    for (used, percentage) in [(0, 0.0), (50_000, 25.0), (200_000, 100.0), (250_000, 125.0)] {
        assert_eq!(limits.context_usage_percent(used), percentage);
    }
}
