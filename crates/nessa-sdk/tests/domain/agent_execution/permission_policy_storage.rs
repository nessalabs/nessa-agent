//! Immutable policy storage discards spare slots without changing ordered equality.
use super::*;

#[test]
fn offer_policy_compacts_spare_slots_and_preserves_clone_equality() {
    let decision = PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request());
    let mut input = Vec::with_capacity(4096);
    input.push(decision.clone());
    let policy = PermissionOfferPolicy::new(input).unwrap();
    let clone = policy.clone();
    assert_eq!(policy, clone);
    assert_eq!(policy.decisions(), &[decision]);
    for value in [policy, clone] {
        let slots = value.decisions.into_vec();
        assert_eq!(slots.capacity(), slots.len());
    }
}
