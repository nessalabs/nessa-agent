use nessa_sdk::domain::common::value_objects::TokenLimits;
use nessa_sdk::domain::common::value_objects::{
    Date,
    ImageMediaType::{self, Gif, Jpeg, Png},
    Url,
};
use nessa_sdk::domain::model_metadata::{
    aggregates::Catalog,
    entities::ModelMetadata,
    value_objects::ImageInputLimits,
    value_objects::ImageInputViolation,
    value_objects::Modalities,
    value_objects::ModelDescription,
    value_objects::ModelFeatures,
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
fn catalog_keeps_its_verification_date_and_reports_domain_failures() {
    let catalog = Catalog::new(verified_on(), vec![model(ModelProvider::OpenAi, false)]).unwrap();
    assert_eq!(catalog.verified_on(), &verified_on());
    let invalid_date = MetadataError::from(Date::new("2026-02-30".into()).unwrap_err());
    let invalid_url = MetadataError::from(Url::new("relative").unwrap_err());
    let cases = [
        (
            Modalities::new(false, false, false).unwrap_err(),
            "modalities: must support at least one modality",
        ),
        (
            MetadataError::from(TokenLimits::new(0, 1).unwrap_err()),
            "maximum context window: must be positive",
        ),
        (
            invalid_date,
            "expected a valid calendar date in YYYY-MM or YYYY-MM-DD format",
        ),
        (
            invalid_url,
            "invalid absolute URL: relative URL without a base",
        ),
        (
            ModelProvider::try_from("unknown").unwrap_err(),
            "unsupported model provider: unknown",
        ),
        (
            Catalog::new(verified_on(), vec![]).unwrap_err(),
            "a model catalog must contain at least one model",
        ),
        (
            Catalog::new(
                verified_on(),
                vec![
                    model(ModelProvider::OpenAi, true),
                    model(ModelProvider::OpenAi, false),
                ],
            )
            .unwrap_err(),
            "duplicate model: openai/test-model",
        ),
    ];
    for (error, message) in cases {
        assert_eq!(error.to_string(), message);
    }
}

#[test]
fn model_description_preserves_human_and_provider_metadata() {
    let model = model(ModelProvider::Anthropic, true);
    assert_eq!(model.description().display_name(), "Test model");
    assert_eq!(model.description().knowledge_cutoff().as_str(), "2026-01");
    assert_eq!(
        model.description().documentation_url().as_str(),
        "https://example.com/model"
    );
    assert_eq!(model.key().provider().as_str(), "anthropic");
}

#[test]
fn provider_names_parse_into_the_closed_provider_set() {
    assert_eq!(ModelProvider::try_from("openai"), Ok(ModelProvider::OpenAi));
    assert_eq!(
        ModelProvider::try_from("anthropic"),
        Ok(ModelProvider::Anthropic)
    );
}

#[test]
fn image_input_limits_are_positive_consistent_and_derive_what_is_worth_sending() {
    let limits = ImageInputLimits::new(vec![Jpeg, Png], 5_000_000, 8000, 2000, 2576).unwrap();
    assert_eq!(limits.media_types(), [Jpeg, Png]);
    assert_eq!(limits.max_encoded_bytes(), 5_000_000);
    // Base64 turns three bytes into four characters.
    assert_eq!(limits.max_raw_bytes(), 3_750_000);
    assert_eq!(limits.max_edge_px(), 8000);
    // All the model sees, but never past the many-image ceiling a long
    // conversation reaches.
    assert_eq!(limits.target_long_edge_px(), 2000);
    let standard = ImageInputLimits::new(vec![Png], 5_000_000, 8000, 2000, 1568).unwrap();
    assert_eq!(standard.target_long_edge_px(), 1568);

    for invalid in [
        ImageInputLimits::new(vec![], 4, 1, 1, 1),
        ImageInputLimits::new(vec![Png, Jpeg, Png], 4, 1, 1, 1),
        ImageInputLimits::new(vec![Png], 0, 1, 1, 1),
        // Fewer than one base64 group would admit no byte at all.
        ImageInputLimits::new(vec![Png], 1, 1, 1, 1),
        ImageInputLimits::new(vec![Png], 3, 1, 1, 1),
        ImageInputLimits::new(vec![Png], 4, 0, 1, 1),
        ImageInputLimits::new(vec![Png], 4, 1, 0, 1),
        ImageInputLimits::new(vec![Png], 4, 1, 1, 0),
        ImageInputLimits::new(vec![Png], 4, 100, 101, 100),
        ImageInputLimits::new(vec![Png], 4, 100, 100, 101),
    ] {
        assert!(
            matches!(
                invalid,
                Err(MetadataError::Invalid {
                    field: "image input",
                    ..
                })
            ),
            "{invalid:?}"
        );
    }
}

#[test]
fn the_smallest_image_limit_still_admits_an_image() {
    let smallest = ImageInputLimits::new(vec![Png], ImageInputLimits::MIN_ENCODED_BYTES, 1, 1, 1);
    assert_eq!(smallest.unwrap().max_raw_bytes(), 3);
}

#[test]
fn an_image_is_checked_against_the_models_encodings_and_byte_limit() {
    // Eight base64 characters carry six bytes.
    let limits = ImageInputLimits::new(vec![Png, Jpeg], 8, 1, 1, 1).unwrap();
    assert_eq!(limits.check(Png, 6), Ok(()));
    assert_eq!(limits.check(Jpeg, 1), Ok(()));
    assert_eq!(
        limits.check(Png, 7),
        Err(ImageInputViolation::TooLarge {
            size: 7,
            max_bytes: 6
        })
    );
    // The encoding is named first: a smaller GIF would be refused as well.
    assert_eq!(
        limits.check(Gif, 7),
        Err(ImageInputViolation::MediaType(Gif))
    );
    // Each says which limit, in words a person can act on.
    assert_eq!(
        limits.check(Gif, 1).unwrap_err().to_string(),
        "the model does not accept image/gif"
    );
    assert_eq!(
        limits.check(Png, 7).unwrap_err().to_string(),
        "an image of 7 bytes exceeds the model's 6"
    );
}

#[test]
fn image_limits_require_the_image_input_modality() {
    let limits = ImageInputLimits::new(vec![ImageMediaType::Png], 4, 1, 1, 1).unwrap();
    let sees_images = model(ModelProvider::Anthropic, true);
    assert_eq!(sees_images.image_input(), None);
    let recorded = sees_images.with_image_input(limits.clone()).unwrap();
    assert_eq!(recorded.image_input(), Some(&limits));
    assert!(matches!(
        model(ModelProvider::Anthropic, false).with_image_input(limits),
        Err(MetadataError::Invalid {
            field: "image input",
            ..
        })
    ));
}
