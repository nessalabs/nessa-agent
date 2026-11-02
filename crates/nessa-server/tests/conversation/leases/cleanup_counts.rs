//! A restored provider session ID does not merge distinct backend cleanup owners.
use std::collections::HashSet;
use uuid::Uuid;

pub(super) fn assert_each_backend_closed_once(original: Uuid, opened: &[Uuid], closed: &[Uuid]) {
    assert_eq!(
        opened.first(),
        Some(&original),
        "the original backend remains represented"
    );
    assert_eq!(
        opened.iter().collect::<HashSet<_>>().len(),
        opened.len(),
        "backend instances have unique identities"
    );
    assert_eq!(
        closed.len(),
        opened.len(),
        "every acquired backend has a cleanup acknowledgement"
    );
    for id in opened {
        assert_eq!(
            closed.iter().filter(|closed| **closed == *id).count(),
            1,
            "each acquired backend has exactly one cleanup acknowledgement: {id}"
        );
    }
}

#[test]
fn two_distinct_backend_instances_each_have_their_own_cleanup() {
    let original = Uuid::from_u128(1);
    let replacement = Uuid::from_u128(2);
    assert_each_backend_closed_once(original, &[original], &[original]);
    assert_each_backend_closed_once(original, &[original, replacement], &[original, replacement]);
}

#[test]
#[should_panic(expected = "each acquired backend has exactly one cleanup acknowledgement")]
fn a_duplicate_original_close_does_not_acknowledge_the_replacement() {
    let original = Uuid::from_u128(1);
    let replacement = Uuid::from_u128(2);
    assert_each_backend_closed_once(original, &[original, replacement], &[original, original]);
}

#[test]
#[should_panic(expected = "every acquired backend has a cleanup acknowledgement")]
fn a_missing_backend_cleanup_is_not_an_acknowledgement() {
    let original = Uuid::from_u128(1);
    let replacement = Uuid::from_u128(2);
    assert_each_backend_closed_once(original, &[original, replacement], &[original]);
}

#[test]
#[should_panic(expected = "each acquired backend has exactly one cleanup acknowledgement")]
fn an_unrelated_close_cannot_substitute_for_the_replacement() {
    let original = Uuid::from_u128(1);
    let replacement = Uuid::from_u128(2);
    let unrelated = Uuid::from_u128(3);
    assert_each_backend_closed_once(original, &[original, replacement], &[original, unrelated]);
}

#[test]
#[should_panic(expected = "the original backend remains represented")]
fn a_replacement_cannot_erase_the_original_backend_identity() {
    let original = Uuid::from_u128(1);
    let replacement = Uuid::from_u128(2);
    assert_each_backend_closed_once(original, &[replacement], &[replacement]);
}

#[test]
#[should_panic(expected = "backend instances have unique identities")]
fn duplicate_acquired_ids_cannot_merge_distinct_cleanup_owners() {
    let original = Uuid::from_u128(1);
    let unrelated = Uuid::from_u128(2);
    assert_each_backend_closed_once(original, &[original, original], &[original, unrelated]);
}

#[test]
#[should_panic(expected = "every acquired backend has a cleanup acknowledgement")]
fn an_extra_unowned_cleanup_is_not_an_acquired_backend_acknowledgement() {
    let original = Uuid::from_u128(1);
    let unrelated = Uuid::from_u128(2);
    assert_each_backend_closed_once(original, &[original], &[original, unrelated]);
}
