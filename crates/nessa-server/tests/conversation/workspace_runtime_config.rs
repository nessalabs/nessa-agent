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

#[test]
fn global_override_map_does_not_apply_a_single_owners_channel_quota() {
    let channels = (0u64..1001)
        .map(|n| {
            let mut bytes = [0; 10];
            for (index, byte) in bytes.iter_mut().enumerate() {
                *byte = ((n >> (index * 5)) & 31) as u8;
            }
            (
                ChannelId::from_random(bytes).as_str().to_owned(),
                Value::Null,
            )
        })
        .collect::<Map<String, Value>>();
    assert_eq!(channels.len(), 1001);
    let bytes =
        serde_json::to_vec(&serde_json::json!({"boat":{"channelAutoStopSeconds":channels}}))
            .unwrap();
    let parsed = BoatRuntimeConfig::from_document(&bytes).unwrap();
    assert_eq!(
        parsed.section()["channelAutoStopSeconds"]
            .as_object()
            .unwrap()
            .len(),
        1001
    );
}
