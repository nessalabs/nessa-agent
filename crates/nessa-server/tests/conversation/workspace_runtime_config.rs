use super::*;

fn channel() -> ChannelId {
    ChannelId::from_random([1; 10])
}

#[test]
fn default_and_explicit_unlimited_override_are_distinct_and_resolve_from_one_document() {
    let id = channel();
    let parsed: BoatRuntimeConfig = serde_json::from_value(serde_json::json!({
        "autoStopSeconds": 14_400,
        "channelAutoStopSeconds": {id.as_str(): null}
    }))
    .unwrap();
    assert_eq!(parsed.snapshot(None).effective().seconds(), Some(14_400));
    assert_eq!(
        parsed.snapshot(Some(&id)).effective(),
        VmLifetime::Unlimited
    );
    assert_eq!(parsed.channel_override(&id), Some(VmLifetime::Unlimited));
    assert_eq!(parsed.default_lifetime().seconds(), Some(14_400));
    let initial: BoatRuntimeConfig = serde_json::from_str("{}").unwrap();
    assert_eq!(
        initial.snapshot(Some(&id)).effective(),
        VmLifetime::Unlimited
    );
    assert_eq!(initial.channel_override(&id), None);
}

#[test]
fn contradictory_and_invalid_configuration_is_refused_before_any_edit() {
    let id = channel();
    for text in [
        r#"{"autoStopSeconds":0}"#.into(),
        r#"{"autoStopSeconds":4294967296}"#.into(),
        r#"{"autoStopSeconds":-1}"#.into(),
        r#"{"autoStopSeconds":1.5}"#.into(),
        r#"{"autoStopSeconds":true}"#.into(),
        r#"{"unknown":null}"#.into(),
        r#"{"autoStopSeconds":null,"autoStopSeconds":3600}"#.into(),
        format!(r#"{{"channelAutoStopSeconds":{{"{id}":null,"{id}":3600}}}}"#),
        format!(r#"{{"channelAutoStopSeconds":{{"{id}":3600,"{id}":null}}}}"#),
        r#"{"channelAutoStopSeconds":{"not-a-channel":null}}"#.into(),
    ] {
        assert!(
            serde_json::from_str::<BoatRuntimeConfig>(&text).is_err(),
            "{text}"
        );
    }
}
