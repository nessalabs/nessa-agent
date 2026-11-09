//! Decoder paths never publish arbitrary map keys or scalar values.
use super::*;
use std::collections::BTreeMap;

#[test]
fn decoder_path_projects_unknown_keys_without_retaining_names_or_values() {
    type Shape = BTreeMap<String, BTreeMap<String, u32>>;
    let body = serde_json::json!({"sandbox":{"PRIVATE_KEY_SECRET":"PRIVATE_VALUE_SECRET"}});
    let diagnostic = decode::<Shape>(&body, Stage::InfoDecode, &["memoryGB"]).unwrap_err();
    let output = format!("{diagnostic:?}");
    assert_eq!(diagnostic.field, "unknown");
    assert_eq!(diagnostic.observed_type, "not_collected");
    assert!(!output.contains("PRIVATE_KEY_SECRET"));
    assert!(!output.contains("PRIVATE_VALUE_SECRET"));
}
