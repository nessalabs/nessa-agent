use super::{VmLifetime, VmLifetimeSettings};
use crate::conversation::domain::ChannelId;

#[test]
fn unlimited_and_finite_lifetimes_preserve_the_requested_provider_value() {
    assert_eq!(VmLifetime::default(), VmLifetime::Unlimited);
    assert_eq!(VmLifetime::from_seconds(None), Some(VmLifetime::Unlimited));
    assert_eq!(VmLifetime::Unlimited.seconds(), None);
    for seconds in [1, 3600, 14_400, 86_400, u64::from(u32::MAX)] {
        assert_eq!(
            VmLifetime::from_seconds(Some(seconds))
                .unwrap()
                .seconds()
                .map(u64::from),
            Some(seconds)
        );
    }
}

#[test]
fn zero_and_unrepresentable_lifetimes_are_refused_instead_of_defaulted() {
    for seconds in [0, u64::from(u32::MAX) + 1, u64::MAX] {
        assert_eq!(VmLifetime::from_seconds(Some(seconds)), None);
    }
}

#[test]
fn inheritance_and_explicit_overrides_resolve_without_turning_unlimited_into_absence() {
    let finite = VmLifetime::from_seconds(Some(14_400)).unwrap();
    for default in [VmLifetime::Unlimited, finite] {
        let inherited =
            VmLifetimeSettings::for_channel(ChannelId::from_random([1; 10]), default, None);
        assert_eq!(inherited.default_lifetime(), default);
        assert_eq!(inherited.channel_override(), None);
        assert_eq!(inherited.effective(), default);
        for lifetime in [VmLifetime::Unlimited, finite] {
            let explicit = VmLifetimeSettings::for_channel(
                ChannelId::from_random([1; 10]),
                default,
                Some(lifetime),
            );
            assert_eq!(explicit.channel_override(), Some(lifetime));
            assert_eq!(explicit.effective(), lifetime);
        }
    }
}
