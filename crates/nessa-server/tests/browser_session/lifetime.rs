use crate::browser_session::domain::value_objects::{Lifetime, IDLE_SECONDS};
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
