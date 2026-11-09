use super::*;
use crate::conversation::domain::ChannelId;

fn peer(byte: u8) -> PeerEnvironment {
    PeerEnvironment::new([byte; PeerEnvironment::KEY_BYTES])
}

#[test]
fn a_computer_named_twice_or_both_ordered_and_left_out_is_refused() {
    assert_eq!(
        PoolPreference::new(vec![peer(1), peer(1)], vec![]),
        Err(PoolRefusal::Repeated)
    );
    assert_eq!(
        PoolPreference::new(vec![peer(1)], vec![peer(1)]),
        Err(PoolRefusal::Repeated)
    );
    let many = (0..=MAX_POOL_ENTRIES as u8).map(peer).collect::<Vec<_>>();
    assert_eq!(PoolPreference::new(many, vec![]), Err(PoolRefusal::TooMany));
}

#[test]
fn an_ordered_computer_goes_before_every_one_not_named() {
    let preference = PoolPreference::new(vec![peer(3), peer(1)], vec![peer(2)]).unwrap();
    assert_eq!(preference.rank(&peer(3)), 0);
    assert_eq!(preference.rank(&peer(1)), 1);
    assert_eq!(preference.rank(&peer(9)), 2);
    assert!(!preference.includes(&peer(2)));
    assert!(preference.includes(&peer(9)));
}

#[test]
fn a_channel_override_replaces_the_default_whole_and_its_absence_inherits() {
    let default = PoolPreference::new(vec![peer(1)], vec![peer(2)]).unwrap();
    let own = PoolPreference::new(vec![peer(2)], vec![]).unwrap();
    let channel = ChannelId::new("ch_aaaaaaaaaa").unwrap();
    let inherits = PoolSettings::new(
        SettingsScope::Channel(channel.clone()),
        default.clone(),
        None,
        900,
        900,
    );
    assert_eq!(inherits.effective(), &default);
    let overridden = PoolSettings::new(
        SettingsScope::Channel(channel),
        default.clone(),
        Some(own.clone()),
        900,
        900,
    );
    assert_eq!(overridden.effective(), &own);
    // A global read never carries an override.
    let global = PoolSettings::new(SettingsScope::Global, default.clone(), Some(own), 900, 900);
    assert_eq!(global.channel_override(), None);
    assert_eq!(global.effective(), &default);
}

#[test]
fn a_computer_at_either_limit_is_busy_and_one_that_never_reported_is_not() {
    let settings = PoolSettings::new(
        SettingsScope::Global,
        PoolPreference::default(),
        None,
        900,
        850,
    );
    assert!(!settings.busy(None, None));
    assert!(!settings.busy(Some(899), Some(849)));
    assert!(settings.busy(Some(900), None));
    assert!(settings.busy(None, Some(850)));
}
