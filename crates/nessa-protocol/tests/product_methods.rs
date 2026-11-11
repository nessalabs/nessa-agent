//! `ProductMethod` is generated from `protocol/product/manifest.json`: these
//! hold it to that file, and its typed params to the shared fixtures.

use nessa_protocol::product::generated::{action_for_method, ProductLane, ProductMethod};
use serde_json::{json, Value};

fn manifest() -> serde_json::Map<String, Value> {
    let manifest: Value =
        serde_json::from_str(include_str!("../../../protocol/product/manifest.json")).unwrap();
    manifest["methods"].as_object().unwrap().clone()
}

#[test]
fn every_manifest_method_is_one_variant_with_its_grant_and_lane() {
    let methods = manifest();
    let names = methods.keys().map(String::as_str).collect::<Vec<_>>();
    let variants = ProductMethod::ALL
        .iter()
        .map(|method| method.as_str())
        .collect::<Vec<_>>();
    assert_eq!(variants, names);
    for (name, spec) in &methods {
        let method = ProductMethod::parse(name).unwrap_or_else(|| panic!("{name}"));
        assert_eq!(method.as_str(), name);
        assert_eq!(method.grant(), spec["grant"].as_str(), "{name}");
        assert_eq!(action_for_method(name), spec["grant"].as_str(), "{name}");
        assert_eq!(
            Some(method.lane().as_str()),
            spec["lane"].as_str(),
            "{name}"
        );
    }
    assert_eq!(ProductMethod::parse("conversation.watchRecords"), None);
    assert_eq!(action_for_method("conversation.watchRecords"), None);
    assert!(ProductLane::ALL
        .iter()
        .all(|lane| methods.values().any(|spec| spec["lane"] == lane.as_str())));
}

#[test]
fn params_decode_into_the_declared_type() {
    let fixtures: Value =
        serde_json::from_str(include_str!("../../../protocol/product/fixtures.json")).unwrap();
    let mut decoded = 0;
    for (name, spec) in manifest() {
        let method = ProductMethod::parse(&name).unwrap();
        // A string is never params. (A derived struct decodes from an array
        // as serde allows; that is the handlers' decode too, unchanged here.)
        assert!(
            method.decode_params(json!("params")).is_err(),
            "{name} took a string"
        );
        match spec["params"].as_str() {
            None => {
                let params = method.decode_params(json!({})).unwrap();
                assert_eq!(params.method(), method);
                assert!(
                    method.decode_params(json!({"unexpected": true})).is_err(),
                    "{name} took a field it declares none of"
                );
            }
            Some(schema) => {
                if let Some(fixture) = fixtures.get(schema) {
                    let params = method
                        .decode_params(fixture.clone())
                        .unwrap_or_else(|_| panic!("{name}: its {schema} fixture"));
                    assert_eq!(params.method(), method);
                    decoded += 1;
                }
            }
        }
    }
    assert!(decoded > 0, "no params fixture was decoded");
}
