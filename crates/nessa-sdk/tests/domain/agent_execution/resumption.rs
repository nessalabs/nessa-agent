//! Successor identities for turns a restart cut off.
use super::*;

fn id(value: &str) -> ExecutionId {
    ExecutionId::new(value).unwrap()
}

#[test]
fn a_chain_has_three_successors_derived_from_the_original() {
    let original = Resumption::of(&id("message-1"));
    assert_eq!(original.attempt(), 0);
    let first = original.successor().unwrap();
    assert!(first.as_str().starts_with("resume-1-"));
    assert_eq!(
        Resumption::of(&id("message-1")).successor(),
        Some(first.clone())
    );
    let second = Resumption::of(&first).successor().unwrap();
    assert!(second.as_str().starts_with("resume-2-"));
    assert_eq!(second.as_str()[9..], first.as_str()[9..]);
    let third = Resumption::of(&second).successor().unwrap();
    assert_eq!(Resumption::of(&third).attempt(), 3);
    assert_eq!(Resumption::of(&third).successor(), None);
}

#[test]
fn an_id_that_only_looks_like_a_successor_is_an_original() {
    for value in [
        "resume-1-short",
        "resume-4-0000000000000000000000000000000000000000000000000000000000000000",
        "resume-01-0000000000000000000000000000000000000000000000000000000000000000",
        "resume-1-000000000000000000000000000000000000000000000000000000000000000G",
        "resume-x-0000000000000000000000000000000000000000000000000000000000000000",
        "resume-0-0000000000000000000000000000000000000000000000000000000000000000",
        "resume-1",
    ] {
        assert_eq!(Resumption::of(&id(value)).attempt(), 0, "{value}");
    }
}

#[test]
fn a_successor_written_in_lowercase_hex_is_read_back_as_one() {
    let value = format!("resume-2-{}", "0123456789abcdef".repeat(4));
    let read = Resumption::of(&id(&value));
    assert_eq!(read.attempt(), 2);
    assert_eq!(
        read.successor().unwrap().as_str(),
        format!("resume-3-{}", "0123456789abcdef".repeat(4))
    );
}

#[test]
fn a_successor_of_the_longest_original_still_fits() {
    let longest = id(&"x".repeat(ExecutionId::MAX_BYTES));
    assert!(Resumption::of(&longest).successor().is_some());
}
