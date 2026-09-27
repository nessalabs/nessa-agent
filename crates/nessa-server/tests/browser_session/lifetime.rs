use crate::browser_session::domain::value_objects::{
    Lifetime, FUTURE_TOLERANCE_SECONDS, IDLE_SECONDS,
};
#[test]
fn rolling_idle_deadlines_preserve_creation() {
    let original = Lifetime::new(100).unwrap();
    let renewed = original.renew(100 + 20 * 86400).unwrap();
    assert_eq!(original.idle_expires_at(), 100 + IDLE_SECONDS);
    assert_eq!(renewed.created_at(), 100);
    assert_eq!(renewed.idle_expires_at(), 100 + 50 * 86400);
    assert!(original.renew(99).is_none());
    assert!(original.renew(original.idle_expires_at()).is_none());
    assert!(Lifetime::restore(100, 99, 100 + IDLE_SECONDS).is_none());
    assert!(Lifetime::restore(100, 100, 100 + IDLE_SECONDS + 1).is_none());
    assert!(Lifetime::new(u64::MAX).is_none());
}

#[test]
fn a_renewal_is_plausible_up_to_one_tolerance_ahead_of_the_clock() {
    let lifetime = Lifetime::new(100_000).unwrap();
    assert!(lifetime.is_plausible_at(100_000));
    assert!(lifetime.is_plausible_at(100_001));
    assert!(lifetime.is_plausible_at(100_000 - FUTURE_TOLERANCE_SECONDS));
    assert!(!lifetime.is_plausible_at(100_000 - FUTURE_TOLERANCE_SECONDS - 1));
    // A clock reading near the end of the range must not wrap into acceptance.
    assert!(Lifetime::new(1).unwrap().is_plausible_at(u64::MAX));
}
