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
            "toolUse": true, "reasoning": false,
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
    for field in ["toolUse", "reasoning"] {
        let mut value = fixture();
        value["models"][0].as_object_mut().unwrap().remove(field);
        assert!(matches!(parse(&value), Err(LoadCatalogError::Json(_))));
    }
    for invalid in [Value::Null, json!("unknown"), json!(1)] {
        let mut value = fixture();
        value["models"][0]["input"]["audio"] = invalid;
        assert!(matches!(parse(&value), Err(LoadCatalogError::Json(_))));
    }
    assert!(!parse(&fixture()).unwrap().models()[0].reasoning);
}

#[test]
fn rejects_unknown_fields_at_every_level() {
    for pointer in ["", "/models/0", "/models/0/input", "/models/0/output"] {
        let mut value = fixture();
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
