//! Immutable policy storage discards spare slots without changing ordered equality.
use super::*;

#[test]
fn a_hook_stop_is_an_end_turn_or_a_close_and_a_continue_is_not() {
    let session = ExecutionSessionId::new("session").unwrap();
    let execution = ExecutionId::new("turn").unwrap();
    let review = PermissionId::new("7").unwrap();
    let stopped = CustomPermissionCancellationReason::hook_refusal(
        &session,
        &execution,
        &review,
        RefusalOutcome::EndTurn,
        "shell is blocked",
    )
    .unwrap();
    assert_eq!(stopped.hook_stop(), Some(RefusalOutcome::EndTurn));
    let closed = CustomPermissionCancellationReason::hook_refusal(
        &session,
        &execution,
        &review,
        RefusalOutcome::CloseSession,
        "close it",
    )
    .unwrap();
    assert_eq!(closed.hook_stop(), Some(RefusalOutcome::CloseSession));
    let continued = CustomPermissionCancellationReason::hook_refusal(
        &session,
        &execution,
        &review,
        RefusalOutcome::Continue,
        "not this tool",
    )
    .unwrap();
    assert_eq!(continued.hook_stop(), None);
    let other = CustomPermissionCancellationReason::new("guard", "withdrawn").unwrap();
    assert_eq!(other.hook_stop(), None);
}

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
