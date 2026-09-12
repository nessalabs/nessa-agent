use nessa_sdk::domain::{
    common::value_objects::{Date, Url},
    effective_capabilities::value_objects::{
        BindingRestrictions, CapabilityError, CapabilityRequirement as R, EffectiveCapabilities,
        Modality as M,
    },
    model_metadata::{
        entities::ModelMetadata,
        value_objects::{
            Modalities, ModelDescription, ModelFeatures, ModelKey, ModelProvider, TokenLimits,
        },
    },
};

fn features(image: bool, audio: bool, tool_use: bool, reasoning: bool) -> ModelFeatures {
    ModelFeatures::new(
        Modalities::new(true, image, audio).unwrap(),
        Modalities::new(true, image, audio).unwrap(),
        tool_use,
        reasoning,
    )
}
fn limits(context: u32, output: u32) -> TokenLimits {
    TokenLimits::new(context, output).unwrap()
}
fn model(id: &str, features: ModelFeatures) -> ModelMetadata {
    ModelMetadata::new(
        ModelKey::new(ModelProvider::OpenAi, id.into()).unwrap(),
        ModelDescription::new(
            "Test".into(),
            Date::new("2026-01".into()).unwrap(),
            Url::new("https://example.com").unwrap(),
        )
        .unwrap(),
        features,
        limits(1000, 200),
    )
}
fn binding(features: ModelFeatures) -> BindingRestrictions {
    BindingRestrictions::new(features, limits(800, 150))
}

#[test]
fn restrictions_only_remove_features_for_every_boolean_combination() {
    for model_mask in 0..16 {
        for binding_mask in 0..16 {
            let f = |mask| features(mask & 1 != 0, mask & 2 != 0, mask & 4 != 0, mask & 8 != 0);
            let model = model("one", f(model_mask));
            let snapshot =
                EffectiveCapabilities::new(&model, binding(f(binding_mask)), limits(700, 100))
                    .unwrap();
            for (bit, requirements) in [
                (1, vec![R::Input(M::Image), R::Output(M::Image)]),
                (2, vec![R::Input(M::Audio), R::Output(M::Audio)]),
                (4, vec![R::ToolUse]),
                (8, vec![R::Reasoning]),
            ] {
                for requirement in requirements {
                    let supported = model_mask & binding_mask & bit != 0;
                    assert_eq!(snapshot.supports(requirement), supported);
                    assert_eq!(
                        snapshot.validate(&[requirement], 10, 10),
                        if supported {
                            Ok(())
                        } else {
                            Err(CapabilityError::Unsupported(requirement))
                        }
                    );
                }
            }
        }
    }
}

#[test]
fn explicit_settings_must_fit_both_model_and_binding_ceilings() {
    let model = model("one", features(true, false, true, false));
    for (binding_limits, configured, field, maximum) in [
        (limits(800, 150), limits(801, 100), "context window", 800),
        (limits(2000, 500), limits(1001, 100), "context window", 1000),
        (limits(800, 150), limits(700, 151), "output", 150),
        (limits(2000, 500), limits(700, 201), "output", 200),
    ] {
        assert!(
            matches!(EffectiveCapabilities::new(&model, BindingRestrictions::new(model.features(), binding_limits), configured), Err(CapabilityError::ConfiguredLimitExceeded { limit, maximum: actual, .. }) if limit == field && actual == maximum)
        );
    }
    let snapshot =
        EffectiveCapabilities::new(&model, binding(model.features()), limits(600, 100)).unwrap();
    assert_eq!(snapshot.limits(), limits(600, 100));
    assert_eq!(snapshot.limits().context_usage_percent(300), 50.0);
    assert_eq!(model.limits(), limits(1000, 200));
}

#[test]
fn token_validation_reserves_output_and_handles_boundaries_and_overflow() {
    let model = model("one", features(false, false, true, false));
    let snapshot =
        EffectiveCapabilities::new(&model, binding(model.features()), limits(600, 100)).unwrap();
    assert_eq!(snapshot.validate(&[R::Input(M::Text)], 500, 100), Ok(()));
    for input in [501, 600, u64::MAX] {
        assert!(matches!(
            snapshot.validate(&[], input, 100),
            Err(CapabilityError::ContextWindowExceeded { .. })
        ));
    }
    for output in [0, 101, u32::MAX] {
        assert!(matches!(
            snapshot.validate(&[], 0, output),
            Err(CapabilityError::InvalidOutputBudget { .. })
        ));
    }
    assert_eq!(
        snapshot.validate(&[R::Input(M::Text), R::Input(M::Image)], 10, 10),
        Err(CapabilityError::Unsupported(R::Input(M::Image)))
    );
}

#[test]
fn incompatible_input_or_output_modalities_fail_construction() {
    let model = model("one", features(false, false, false, false));
    let audio = Modalities::new(false, false, true).unwrap();
    let text = Modalities::new(true, false, false).unwrap();
    for (input, output, expected) in [
        (audio, text, CapabilityError::NoInputModality),
        (text, audio, CapabilityError::NoOutputModality),
    ] {
        assert_eq!(
            EffectiveCapabilities::new(
                &model,
                binding(ModelFeatures::new(input, output, false, false)),
                limits(600, 100)
            ),
            Err(expected)
        );
    }
}

#[test]
fn snapshots_remain_independent_when_model_or_configuration_changes() {
    let first_model = model("first", features(true, false, true, false));
    let second_model = model("second", features(false, true, false, true));
    let unrestricted = binding(features(true, true, true, true));
    let first = EffectiveCapabilities::new(&first_model, unrestricted, limits(600, 100)).unwrap();
    let changed = EffectiveCapabilities::new(
        &first_model,
        binding(features(false, false, false, false)),
        limits(400, 50),
    )
    .unwrap();
    let second = EffectiveCapabilities::new(&second_model, unrestricted, limits(700, 100)).unwrap();
    assert!(first.supports(R::Input(M::Image)));
    assert!(!changed.supports(R::Input(M::Image)));
    assert!(!second.supports(R::Input(M::Image)));
    assert!(second.supports(R::Input(M::Audio)));
    assert_eq!(first.limits().max_context_window(), 600);
    assert_eq!(changed.limits().max_context_window(), 400);
    assert_ne!(first.model(), second.model());
    assert!(first_model.features().input().image());
}

#[test]
fn input_and_output_intersect_independently_for_all_nonempty_modality_sets() {
    let modalities = |mask| Modalities::new(mask & 1 != 0, mask & 2 != 0, mask & 4 != 0).unwrap();
    for model_input in 1..8 {
        for model_output in 1..8 {
            for binding_input in 1..8 {
                for binding_output in 1..8 {
                    let model = model(
                        "modalities",
                        ModelFeatures::new(
                            modalities(model_input),
                            modalities(model_output),
                            false,
                            false,
                        ),
                    );
                    let result = EffectiveCapabilities::new(
                        &model,
                        binding(ModelFeatures::new(
                            modalities(binding_input),
                            modalities(binding_output),
                            false,
                            false,
                        )),
                        limits(600, 100),
                    );
                    let input = model_input & binding_input;
                    let output = model_output & binding_output;
                    if input == 0 {
                        assert_eq!(result, Err(CapabilityError::NoInputModality));
                    } else if output == 0 {
                        assert_eq!(result, Err(CapabilityError::NoOutputModality));
                    } else {
                        let snapshot = result.unwrap();
                        let mut accepted = vec![];
                        for (bit, modality) in [(1, M::Text), (2, M::Image), (4, M::Audio)] {
                            for (mask, requirement) in
                                [(input, R::Input(modality)), (output, R::Output(modality))]
                            {
                                assert_eq!(snapshot.supports(requirement), mask & bit != 0);
                                if mask & bit != 0 {
                                    accepted.push(requirement);
                                } else {
                                    assert_eq!(
                                        snapshot.validate(&[requirement], 1, 1),
                                        Err(CapabilityError::Unsupported(requirement))
                                    );
                                }
                            }
                        }
                        assert_eq!(snapshot.validate(&accepted, 500, 100), Ok(()));
                    }
                }
            }
        }
    }
}

#[test]
fn exact_execution_ceilings_and_smallest_budget_are_valid() {
    let model = model("one", features(true, true, true, true));
    for (binding_limits, configured) in [
        (limits(800, 150), limits(800, 150)),
        (limits(2000, 500), limits(1000, 200)),
        (limits(1, 1), limits(1, 1)),
    ] {
        let snapshot = EffectiveCapabilities::new(
            &model,
            BindingRestrictions::new(model.features(), binding_limits),
            configured,
        )
        .unwrap();
        assert_eq!(snapshot.limits(), configured);
        assert_eq!(
            snapshot.validate(
                &[
                    R::Input(M::Text),
                    R::Output(M::Audio),
                    R::ToolUse,
                    R::Reasoning
                ],
                u64::from(configured.max_context_window() - configured.max_output()),
                configured.max_output()
            ),
            Ok(())
        );
        assert_eq!(snapshot.validate(&[], 0, 1), Ok(()));
    }
}

#[test]
fn validation_reports_the_first_unsupported_requirement_before_budget_errors() {
    let model = model("one", features(false, false, false, false));
    let snapshot =
        EffectiveCapabilities::new(&model, binding(model.features()), limits(600, 100)).unwrap();
    assert_eq!(
        snapshot.validate(
            &[
                R::Input(M::Text),
                R::Output(M::Text),
                R::ToolUse,
                R::Reasoning
            ],
            u64::MAX,
            0
        ),
        Err(CapabilityError::Unsupported(R::ToolUse))
    );
    assert_eq!(
        snapshot.validate(&[R::Reasoning, R::ToolUse], 0, 1),
        Err(CapabilityError::Unsupported(R::Reasoning))
    );
    assert_eq!(
        snapshot.validate(&[R::Input(M::Text), R::Input(M::Text)], 599, 1),
        Ok(())
    );
}

#[test]
fn errors_explain_unsupported_inputs_and_budget_configuration_without_provider_access() {
    let model = model("one", features(false, false, false, false));
    let snapshot =
        EffectiveCapabilities::new(&model, binding(model.features()), limits(600, 100)).unwrap();
    let audio = Modalities::new(false, false, true).unwrap();
    let text = model.features().input();
    let build = |input, output, configured| {
        EffectiveCapabilities::new(
            &model,
            binding(ModelFeatures::new(input, output, false, false)),
            configured,
        )
        .unwrap_err()
    };
    let cases = [
        (
            build(text, text, limits(801, 100)),
            "configured context window 801 exceeds execution ceiling 800",
        ),
        (
            build(text, text, limits(600, 151)),
            "configured output 151 exceeds execution ceiling 150",
        ),
        (
            build(audio, text, limits(600, 100)),
            "model and binding share no input modality",
        ),
        (
            build(text, audio, limits(600, 100)),
            "model and binding share no output modality",
        ),
        (
            snapshot.validate(&[R::Input(M::Image)], 1, 1).unwrap_err(),
            "unsupported capability: Input(Image)",
        ),
        (
            snapshot.validate(&[], 0, 101).unwrap_err(),
            "output budget 101 must be positive and at most 100",
        ),
        (
            snapshot.validate(&[], 501, 100).unwrap_err(),
            "input tokens 501 plus output budget 100 exceed context window 600",
        ),
    ];
    for (error, message) in cases {
        assert_eq!(error.to_string(), message);
        assert!(std::error::Error::source(&error).is_none());
    }
}
