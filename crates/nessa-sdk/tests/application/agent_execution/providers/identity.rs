//! Exact identity byte bounds and compact ownership at the application boundary.
use super::*;

#[test]
fn provider_identity_bounds_are_utf8_bytes_and_never_truncate_or_normalize() {
    let fields = [
        ProviderIdentity::MAX_NAME_BYTES,
        ProviderIdentity::MAX_MODEL_ID_BYTES,
        ProviderIdentity::MAX_CONTEXT_BYTES,
    ];
    for (index, limit) in fields.into_iter().enumerate() {
        let mut components = [
            "provider".to_owned(),
            "model".to_owned(),
            "context".to_owned(),
        ];
        components[index] = "é".repeat(limit / 2);
        let [name, model, context] = components.clone();
        let identity = ProviderIdentity::new(name, model, context).unwrap();
        assert_eq!(
            [identity.name(), identity.model_id(), identity.context()][index],
            components[index]
        );
        components[index].push('x');
        let [name, model, context] = components;
        assert!(matches!(
            ProviderIdentity::new(name, model, context),
            Err(AgentError::Configuration(_))
        ));
    }
    for (name, model, context) in [
        ("", "model", ""),
        ("provider", "", ""),
        ("bad\nname", "model", ""),
        ("provider", "bad\tmodel", ""),
        ("provider", "model", "bad\0context"),
    ] {
        assert!(ProviderIdentity::new(name, model, context).is_err());
    }
    assert_eq!(
        ProviderIdentity::new("provider", "model", "")
            .unwrap()
            .context(),
        ""
    );
}

#[test]
fn provider_identity_discards_spare_capacity_before_retention_and_cloning() {
    let reserved = || {
        let mut input = String::with_capacity(1024 * 1024);
        input.push_str("exact");
        input
    };
    let identity = ProviderIdentity::new(reserved(), reserved(), reserved()).unwrap();
    for value in [identity.clone(), identity] {
        for field in [value.name, value.model_id, value.context] {
            let compact = field.into_string();
            assert_eq!(compact, "exact");
            assert_eq!(compact.capacity(), compact.len());
        }
    }
}
