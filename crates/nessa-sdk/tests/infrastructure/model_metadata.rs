use nessa_sdk::{
    application::model_catalog::{CatalogError, ModelCatalog},
    infrastructure::model_metadata_json::{load_catalog, LoadCatalogError},
};
use serde_json::{json, Value};
use std::{fs::File, io};

fn fixture() -> Value {
    json!({
        "verifiedOn": "2026-09-11",
        "models": [{
            "provider": "openai", "modelId": "test-model", "displayName": "Test model",
            "input": { "text": true, "image": true, "audio": false },
            "output": { "text": true, "image": false, "audio": false },
            "toolUse": true, "reasoning": null, "fastMode": false,
            "maxContextWindowTokens": 1000, "maxOutputTokens": 100,
            "knowledgeCutoff": "2026-01", "documentationUrl": "https://example.com/model"
        }]
    })
}

fn parse(value: &Value) -> Result<ModelCatalog, LoadCatalogError> {
    load_catalog(value.to_string().as_bytes())
}

#[test]
fn shipped_catalog_loads_from_a_file_and_contains_only_the_current_lineup() {
    let file = File::open(concat!(env!("CARGO_MANIFEST_DIR"), "/data/models.json")).unwrap();
    let catalog = load_catalog(file).unwrap();
    let models = catalog.models();
    let keys: Vec<_> = models
        .iter()
        .map(|m| (m.provider.as_str(), m.model_id.as_str()))
        .collect();
    assert_eq!(
        keys,
        [
            ("openai", "gpt-6-astra"),
            ("openai", "gpt-5.6-sol"),
            ("openai", "gpt-5.6-terra"),
            ("openai", "gpt-5.6-luna"),
            ("anthropic", "claude-fable-5-1"),
            ("anthropic", "claude-opus-5"),
            ("anthropic", "claude-sonnet-5"),
            ("anthropic", "claude-haiku-4-5-20251001"),
            ("opencode", "opencode/minimax-m3"),
            ("opencode", "opencode/nemotron-3-ultra-free"),
            ("opencode", "opencode/big-pickle"),
            ("opencode", "opencode/mimo-v2.5-free"),
        ]
    );
    let haiku = catalog
        .select("anthropic", "claude-haiku-4-5-20251001")
        .unwrap();
    assert_eq!(
        (haiku.max_context_window_tokens, haiku.max_output_tokens),
        (200_000, 64_000)
    );
    assert!(haiku.input.image);
    assert!(!haiku.input.audio);

    // An OpenCode Zen model is named the way Opencode itself names it, slash
    // and all, because the binding sends this string back as the session's
    // `model` option and Opencode matches it against its own list. A bare
    // `big-pickle` would be a model it does not offer.
    let pickle = catalog.select("opencode", "opencode/big-pickle").unwrap();
    assert_eq!(
        (pickle.max_context_window_tokens, pickle.max_output_tokens),
        (200_000, 32_000)
    );
    // Numbers read out of the pinned 1.18.31 binary's own catalogue, which is
    // why they are asserted: they are a fact about the release Nessa pins, so
    // a pin that moves and changes them should fail here rather than quietly
    // resize somebody's context window.
    let nemotron = catalog
        .select("opencode", "opencode/nemotron-3-ultra-free")
        .unwrap();
    assert_eq!(
        (
            nemotron.max_context_window_tokens,
            nemotron.max_output_tokens
        ),
        (1_000_000, 128_000)
    );
    // Image limits are recorded for every Claude model and differ only in the
    // resolution the model sees. OpenAI's are not recorded, so those models are
    // offered no images however their modality flag reads.
    for model in models {
        let limits = model.image_input.as_ref();
        if model.provider != "anthropic" {
            assert_eq!(limits, None, "{}", model.model_id);
            continue;
        }
        let limits = limits.unwrap();
        assert_eq!(
            limits.media_types,
            ["image/jpeg", "image/png", "image/gif", "image/webp"]
        );
        assert_eq!(
            (
                limits.max_encoded_bytes,
                limits.max_edge_px,
                limits.many_images_max_edge_px
            ),
            (5_000_000, 8000, 2000)
        );
        let native = if model.model_id.starts_with("claude-haiku-4-5") {
            1568
        } else {
            2576
        };
        assert_eq!(limits.native_long_edge_px, native, "{}", model.model_id);
    }
}

#[test]
fn each_feature_requires_an_explicit_boolean_including_false() {
    for section in ["input", "output"] {
        for field in ["text", "image", "audio"] {
            let mut value = fixture();
            value["models"][0][section]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(matches!(parse(&value), Err(LoadCatalogError::Json(_))));
        }
    }
    // `reasoning` is required even for a model that does not reason, where it
    // is `null`: an entry that leaves it out is refused, not read as `null`.
    for field in ["toolUse", "reasoning", "fastMode"] {
        let mut value = fixture();
        value["models"][0].as_object_mut().unwrap().remove(field);
        assert!(
            matches!(parse(&value), Err(LoadCatalogError::Json(_))),
            "{field}"
        );
    }
    let mut value = fixture();
    value["models"][0]["reasoning"] = json!({});
    assert!(matches!(parse(&value), Err(LoadCatalogError::Json(_))));
    for invalid in [Value::Null, json!("unknown"), json!(1)] {
        let mut value = fixture();
        value["models"][0]["input"]["audio"] = invalid;
        assert!(matches!(parse(&value), Err(LoadCatalogError::Json(_))));
    }
    assert_eq!(parse(&fixture()).unwrap().models()[0].reasoning, None);
}

#[test]
fn rejects_unknown_fields_at_every_level() {
    for pointer in [
        "",
        "/models/0",
        "/models/0/input",
        "/models/0/output",
        "/models/0/reasoning",
    ] {
        let mut value = fixture();
        value["models"][0]["reasoning"] = json!({ "effortLevels": ["low"] });
        value
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("typo".into(), json!(true));
        assert!(matches!(parse(&value), Err(LoadCatalogError::Json(_))));
    }
}

#[test]
fn duplicate_identity_is_an_error_but_same_model_id_under_another_provider_is_distinct() {
    let mut value = fixture();
    let duplicate = value["models"][0].clone();
    value["models"].as_array_mut().unwrap().push(duplicate);
    assert!(matches!(
        parse(&value),
        Err(LoadCatalogError::Invalid(CatalogError::Duplicate { .. }))
    ));
    value["models"][1]["provider"] = json!("anthropic");
    value["models"][1]["input"]["image"] = json!(false);
    let catalog = parse(&value).unwrap();
    assert!(catalog.select("openai", "test-model").unwrap().input.image);
    assert!(
        !catalog
            .select("anthropic", "test-model")
            .unwrap()
            .input
            .image
    );
}

#[test]
fn missing_model_is_an_actionable_error_without_substitution() {
    let catalog = parse(&fixture()).unwrap();
    for (provider, model) in [
        ("anthropic", "test-model"),
        ("openai", "test-model-latest"),
        ("openai", "TEST-MODEL"),
    ] {
        let error = catalog.select(provider, model).unwrap_err();
        assert!(matches!(error, CatalogError::ModelNotFound { .. }));
        assert!(error
            .to_string()
            .contains("add an entry to the catalog and restart"));
    }
}

#[test]
fn rejects_malformed_descriptive_data_and_impossible_limits() {
    for (pointer, invalid) in [
        ("/models", json!([])),
        ("/verifiedOn", json!("2026-02-29")),
        ("/verifiedOn", json!("2026-09")),
        ("/models/0/provider", json!("")),
        ("/models/0/modelId", json!(" test-model")),
        ("/models/0/displayName", json!("  ")),
        ("/models/0/documentationUrl", json!("http://example.com")),
        ("/models/0/knowledgeCutoff", json!("2026-13")),
        ("/models/0/maxContextWindowTokens", json!(0)),
        ("/models/0/maxOutputTokens", json!(0)),
        ("/models/0/maxOutputTokens", json!(1001)),
        (
            "/models/0/input",
            json!({"text":false,"image":false,"audio":false}),
        ),
        (
            "/models/0/output",
            json!({"text":false,"image":false,"audio":false}),
        ),
    ] {
        let mut value = fixture();
        *value.pointer_mut(pointer).unwrap() = invalid;
        assert!(
            matches!(
                parse(&value),
                Err(LoadCatalogError::Invalid(CatalogError::Invalid { .. }))
            ),
            "{pointer}"
        );
    }
    for invalid in [json!(-1), json!(1.5), json!(4_294_967_296_u64)] {
        let mut value = fixture();
        value["models"][0]["maxContextWindowTokens"] = invalid;
        assert!(matches!(parse(&value), Err(LoadCatalogError::Json(_))));
    }
    let mut value = fixture();
    value["verifiedOn"] = json!("2024-02-29");
    assert!(parse(&value).is_ok());
}

#[test]
fn separate_startup_loads_remain_isolated_and_do_not_refresh() {
    let mut source = fixture();
    let first = parse(&source).unwrap();
    source["models"][0]["input"]["image"] = json!(false);
    let second = parse(&source).unwrap();
    assert!(first.select("openai", "test-model").unwrap().input.image);
    assert!(!second.select("openai", "test-model").unwrap().input.image);
    let mut detached_entry = first.models()[0].clone();
    detached_entry.input.image = false;
    assert!(first.models()[0].input.image);
}

#[test]
fn accepts_injected_application_data_without_the_json_adapter() {
    let source = parse(&fixture()).unwrap();
    let mut entries = source.models().to_vec();
    entries[0].input.image = false;
    let injected = ModelCatalog::from_metadata("2026-09-11".into(), entries).unwrap();
    assert!(!injected.models()[0].input.image);
    assert!(source.models()[0].input.image);
}

#[test]
fn read_failures_bad_json_and_duplicate_fields_never_produce_a_partial_catalog() {
    struct BrokenReader;
    impl io::Read for BrokenReader {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("unreadable source"))
        }
    }
    let Err(LoadCatalogError::Json(error)) = load_catalog(BrokenReader) else {
        panic!("expected read error")
    };
    assert!(error.is_io());
    for source in [
        "{",
        "{}",
        r#"{"verifiedOn":"2026-09-11","verifiedOn":"2026-09-12","models":[]}"#,
    ] {
        assert!(matches!(
            load_catalog(source.as_bytes()),
            Err(LoadCatalogError::Json(_))
        ));
    }
    let trailing = format!("{} {{}}", fixture());
    assert!(matches!(
        load_catalog(trailing.as_bytes()),
        Err(LoadCatalogError::Json(_))
    ));
}

#[test]
fn unsupported_provider_is_rejected_during_import_and_selection() {
    let catalog = parse(&fixture()).unwrap();
    for provider in ["unknown", "OpenAI", "claude", "chatgpt", " openai"] {
        let mut value = fixture();
        value["models"][0]["provider"] = json!(provider);
        assert!(parse(&value).is_err());
        assert!(matches!(
            catalog.select(provider, "test-model"),
            Err(CatalogError::Invalid { .. })
        ));
    }
}

/// The shape `reasoning` had before it carried levels is not read: one
/// current contract, no second reader for the old boolean.
#[test]
fn reasoning_is_null_or_its_options_never_a_boolean() {
    for invalid in [json!(true), json!(false), json!(["low"]), json!("low")] {
        let mut value = fixture();
        value["models"][0]["reasoning"] = invalid.clone();
        assert!(
            matches!(parse(&value), Err(LoadCatalogError::Json(_))),
            "{invalid}"
        );
    }
    let mut value = fixture();
    value["models"][0]["reasoning"] = json!({ "effortLevels": ["low", "xhigh", "max"] });
    value["models"][0]["fastMode"] = json!(true);
    let catalog = parse(&value).unwrap();
    let model = &catalog.models()[0];
    assert_eq!(
        model.reasoning.as_ref().unwrap().effort_levels,
        ["low", "xhigh", "max"]
    );
    assert!(model.fast_mode);
}

#[test]
fn malformed_effort_levels_fail_loading_as_invalid_metadata() {
    for levels in [
        json!(["low", "low"]),
        json!(["High"]),
        json!([""]),
        json!(["a".repeat(33)]),
        json!((0..17)
            .map(|index| format!("level-{index}"))
            .collect::<Vec<_>>()),
    ] {
        let mut value = fixture();
        value["models"][0]["reasoning"] = json!({ "effortLevels": levels.clone() });
        assert!(
            matches!(
                parse(&value),
                Err(LoadCatalogError::Invalid(CatalogError::Invalid { .. }))
            ),
            "{levels}"
        );
    }
}

/// What the providers publish, as recorded on the catalogue's verification
/// date (ADR 302). OpenAI's effort levels were rechecked on 2026-10-08
/// against the reasoning guide and each of those four model pages (#312):
/// Astra is `low` through `max`, and the GPT-5.6 models are `none` through
/// `max`. Those pages do not publish `ultra`. That pass did not recheck the
/// rest of the catalog, so `verifiedOn` stays 2026-09-29. Asserted so that a
/// change to a model's levels, its fast mode, or the catalog-wide date is
/// made on purpose, with its source, rather than slipped in.
#[test]
fn shipped_catalog_records_each_models_published_levels_and_fast_mode() {
    let file = File::open(concat!(env!("CARGO_MANIFEST_DIR"), "/data/models.json")).unwrap();
    let catalog = load_catalog(file).unwrap();
    assert_eq!(catalog.verified_on(), "2026-09-29");
    let openai_5_6 = &["none", "low", "medium", "high", "xhigh", "max"][..];
    let five = &["low", "medium", "high", "xhigh", "max"][..];
    let unrecorded = &[][..];
    for (model_id, levels, fast_mode) in [
        ("gpt-6-astra", five, true),
        ("gpt-5.6-sol", openai_5_6, true),
        ("gpt-5.6-terra", openai_5_6, true),
        ("gpt-5.6-luna", openai_5_6, true),
        ("claude-fable-5-1", five, false),
        ("claude-opus-5", five, true),
        ("claude-sonnet-5", five, false),
        ("claude-haiku-4-5-20251001", unrecorded, false),
        ("opencode/minimax-m3", unrecorded, false),
        ("opencode/nemotron-3-ultra-free", unrecorded, false),
        ("opencode/big-pickle", unrecorded, false),
        ("opencode/mimo-v2.5-free", unrecorded, false),
    ] {
        let model = catalog
            .models()
            .into_iter()
            .find(|model| model.model_id == model_id)
            .unwrap();
        let reasoning = model.reasoning.as_ref().expect(model_id);
        assert_eq!(reasoning.effort_levels, levels, "{model_id}");
        assert_eq!(model.fast_mode, fast_mode, "{model_id}");
    }
}
