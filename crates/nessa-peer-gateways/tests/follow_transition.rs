//! Failure evidence follows the last completed effect, not a later reload.
use super::*;
use std::cell::Cell;

#[test]
fn final_marker_failure_keeps_the_original_rule_and_completed_reset() {
    let before = FollowRule::none();
    let after = FollowRule::new(
        vec!["00000000-0000-4000-8000-000000000001".into()],
        nessa_client_core::retained::FOLLOW_KEEP_MAX,
    )
    .unwrap();
    let removed = Cell::new(false);
    let result = apply_follow_transition(
        Some(before.clone()),
        &after,
        |done| {
            if done {
                Err(PrivateStateError::Unavailable)
            } else {
                Ok(())
            }
        },
        || {
            removed.set(true);
            Ok(())
        },
    );
    assert!(removed.get());
    assert_eq!(
        result,
        Err(FollowFailure::Transition {
            cause: PrivateStateError::Unavailable,
            before: Some(before),
            replica_reset: true,
            replica_reset_durable: true,
        })
    );
}

#[test]
fn failed_reset_preserves_the_before_rule_without_claiming_deletion() {
    let before = FollowRule::none();
    let result = apply_follow_transition(
        Some(before.clone()),
        &before,
        |done| {
            assert!(!done);
            Ok(())
        },
        || {
            Err(CacheRemovalFailure {
                cause: PrivateStateError::Unavailable,
                replica_reset: false,
            })
        },
    );
    assert_eq!(
        result,
        Err(FollowFailure::Transition {
            cause: PrivateStateError::Unavailable,
            before: Some(before),
            replica_reset: false,
            replica_reset_durable: false,
        })
    );
}
