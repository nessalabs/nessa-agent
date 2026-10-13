//! Generated phone DTOs retain original evidence without replacing Auth policy.
use nessa_protocol::product::generated::{
    PhoneAuthorizationStatus, PhoneAuthorizeParams, PhoneDiscovery, PhoneOriginal,
    PhoneStatusParams,
};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};

fn fixture(name: &str) -> Value {
    let values: Value =
        serde_json::from_str(include_str!("../../../protocol/product/fixtures.json")).unwrap();
    values[name].clone()
}
fn roundtrip<T: DeserializeOwned + Serialize>(value: Value) {
    let decoded: T = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), value);
}
#[test]
fn generated_phone_roundtrip_retains_original_and_terminal_cleanup_facts() {
    roundtrip::<PhoneDiscovery>(fixture("PhoneDiscovery"));
    roundtrip::<PhoneOriginal>(fixture("PhoneOriginal"));
    roundtrip::<PhoneAuthorizeParams>(fixture("PhoneAuthorizeParams"));
    roundtrip::<PhoneStatusParams>(fixture("PhoneStatusParams"));
    let active = fixture("PhoneAuthorizationStatus");
    roundtrip::<PhoneAuthorizationStatus>(active.clone());
    let mut terminal = active;
    terminal["phase"] = json!("terminal");
    terminal["terminal"] = json!({"cause":"credential_revoked","initiator":{"kind":"system"}});
    terminal["cleanupPending"] = json!(true);
    terminal["activationStopped"] = json!("permanent");
    roundtrip::<PhoneAuthorizationStatus>(terminal);
    let mut approved = fixture("PhoneAuthorizationStatus");
    approved["phase"] = json!("approved");
    approved.as_object_mut().unwrap().remove("credentialId");
    approved.as_object_mut().unwrap().remove("receiver");
    roundtrip::<PhoneAuthorizationStatus>(approved);
}
#[test]
fn generated_phone_decode_refuses_missing_original_unknown_fields_and_wrong_device_width() {
    for field in ["original", "signature"] {
        let mut value = fixture("PhoneAuthorizeParams");
        value.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<PhoneAuthorizeParams>(value).is_err());
    }
    for field in ["token", "privateKey", "code"] {
        let mut value = fixture("PhoneAuthorizeParams");
        value[field] = json!("not-wire-evidence");
        assert!(serde_json::from_value::<PhoneAuthorizeParams>(value).is_err());
    }
    for width in [31, 33] {
        let mut value = fixture("PhoneAuthorizeParams");
        value["original"]["deviceKey"] = json!(vec![2; width]);
        assert!(serde_json::from_value::<PhoneAuthorizeParams>(value).is_err());
    }
    let mut singular = fixture("PhoneAuthorizationStatus");
    singular["original"]["discovery"]["consent"] = json!({
        "class":"gateway-owner-conversation-read-write",
        "grant":{"action":"conversation.read","resource":{"organizationId":"organization","id":"gateway"}}
    });
    assert!(serde_json::from_value::<PhoneAuthorizationStatus>(singular).is_err());
}
