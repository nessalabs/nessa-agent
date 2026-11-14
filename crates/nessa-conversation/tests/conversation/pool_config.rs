use super::*;

fn name(byte: u8) -> String {
    PeerEnvironment::new([byte; PeerEnvironment::KEY_BYTES]).name()
}

fn document(pool: Value) -> Map<String, Value> {
    serde_json::json!({"pool": pool, "sshHosts": ["kept"]})
        .as_object()
        .unwrap()
        .clone()
}

#[test]
fn an_absent_section_puts_every_computer_in_the_pool_with_none_first() {
    let config = PoolConfig::from_document(&Map::new()).unwrap();
    let settings = config.snapshot(&SettingsScope::Global);
    assert_eq!(settings.effective(), &PoolPreference::default());
    assert!(settings.busy(Some(900), None));
    assert!(!settings.busy(Some(899), Some(899)));
}

#[test]
fn a_channel_reads_its_own_preference_and_another_inherits() {
    let channel = ChannelId::new("ch_aaaaaaaaaa").unwrap();
    let other = ChannelId::new("ch_bbbbbbbbbb").unwrap();
    let config = PoolConfig::from_document(&document(serde_json::json!({
        "order": [name(1)],
        "channels": {channel.as_str(): {"order": [name(2)], "excluded": [name(1)]}},
        "busyCpuPercent": 50,
    })))
    .unwrap();
    let own = config.snapshot(&SettingsScope::Channel(channel));
    assert_eq!(own.effective().order().len(), 1);
    assert_eq!(own.effective().order()[0].name(), name(2));
    assert!(own.busy(Some(500), None));
    let inherited = config.snapshot(&SettingsScope::Channel(other));
    assert_eq!(inherited.channel_override(), None);
    assert_eq!(inherited.effective().order()[0].name(), name(1));
}

#[test]
fn a_bad_name_a_repeat_or_a_bad_limit_refuses_the_section() {
    for pool in [
        serde_json::json!({"order": ["peer:nothex"]}),
        serde_json::json!({"order": [name(1)], "excluded": [name(1)]}),
        serde_json::json!({"busyCpuPercent": 0}),
        serde_json::json!({"busyMemoryPercent": 101}),
        serde_json::json!({"channels": {"default": {"order": []}}}),
        serde_json::json!({"unknown": true}),
    ] {
        assert!(
            PoolConfig::from_document(&document(pool.clone())).is_err(),
            "{pool}"
        );
    }
}

#[test]
fn an_edit_keeps_every_other_key_and_a_removed_override_inherits_again() {
    let channel = ChannelId::new("ch_aaaaaaaaaa").unwrap();
    let mut doc = document(serde_json::json!({"busyCpuPercent": 70}));
    let own = PoolPreference::new(
        vec![PeerEnvironment::new([2; PeerEnvironment::KEY_BYTES])],
        vec![],
    )
    .unwrap();
    edit(
        &mut doc,
        &SettingsScope::Channel(channel.clone()),
        Some(&own),
    )
    .unwrap();
    let config = PoolConfig::from_document(&doc).unwrap();
    assert_eq!(
        config.chosen(&SettingsScope::Channel(channel.clone())),
        Some(own)
    );
    assert_eq!(doc["sshHosts"], serde_json::json!(["kept"]));
    assert_eq!(doc["pool"]["busyCpuPercent"], serde_json::json!(70));
    edit(&mut doc, &SettingsScope::Channel(channel.clone()), None).unwrap();
    let config = PoolConfig::from_document(&doc).unwrap();
    assert_eq!(config.chosen(&SettingsScope::Channel(channel)), None);
}
