//! Capped copies preserve normalized JSON and stop before output growth.
use super::*;
use serde_json::Value;

#[test]
fn canonical_copy_accepts_exact_bound_and_refuses_normalization_growth() {
    let value: Value = serde_json::from_str("1e3").unwrap();
    assert_eq!(bounded_json(&value, 6).ok().as_deref(), Some("1000.0"));
    assert!(matches!(
        bounded_json(&value, 5),
        Err(SerializationFailure::TooLarge)
    ));
    assert!(matches!(
        bounded_json(&value, 3),
        Err(SerializationFailure::TooLarge)
    ));
    assert_eq!(
        bounded_json(&serde_json::json!({"unicode":"😀\n"}), 20)
            .ok()
            .as_deref(),
        Some("{\"unicode\":\"😀\\n\"}")
    );
}
#[test]
fn cap_applies_before_geometric_buffer_growth_and_survives_refusal() {
    let mut bytes = Bytes {
        value: Vec::new(),
        maximum: 13,
        exceeded: false,
    };
    for _ in 0..13 {
        bytes.write_all(b"x").unwrap();
        assert!(bytes.value.capacity() <= 13);
    }
    assert!(bytes.write_all(b"x").is_err());
    assert!(bytes.exceeded);
    assert_eq!(bytes.value, b"xxxxxxxxxxxxx");
    assert!(bytes.value.capacity() <= 13);
}

struct Refuses;
impl Serialize for Refuses {
    fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(<S::Error as serde::ser::Error>::custom(
            "injected serialization refusal",
        ))
    }
}
#[test]
fn non_bound_serialization_failure_is_typed_and_next_copy_remains_usable() {
    assert!(matches!(
        bounded_json(&Refuses, 32),
        Err(SerializationFailure::Invalid)
    ));
    assert_eq!(bounded_json(&true, 4).ok().as_deref(), Some("true"));
}
