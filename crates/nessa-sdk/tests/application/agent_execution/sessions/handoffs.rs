//! Handoffs: the values one holds, the rules a switch or a fork is recorded
//! and taken under, the history a turn after one is given, and the fold that
//! records one and takes it back.

use super::super::test_support::{record_log, snapshot, tool_call, turn};
use super::*;
use crate::application::agent_execution::{
    executions::ExecutionEvent,
    sessions::{records::continuation::Continuation, retained, SessionChange},
};
use crate::domain::agent_execution::{
    executions::MessageChunk,
    prompts::{PromptText, UserMessage},
};

fn actor(request: &str) -> ActionContext {
    ActionContext::new("person", "desktop", request).unwrap()
}
fn identity(name: &str, model: &str) -> ProviderIdentity {
    ProviderIdentity::new(name, model, "").unwrap()
}
fn switch(request: &str, after: Option<&str>, to: ProviderIdentity) -> HandoffRecord {
    HandoffRecord {
        id: HandoffId::of_request(&actor(request)),
        kind: HandoffKind::Switch {
            before: identity("provider", "model"),
            after: to,
        },
        actor: actor(request),
        at_ms: 7,
        after_execution: after.map(|turn| ExecutionId::new(turn).unwrap()),
    }
}
fn history(turns: &[(&str, &str)]) -> CarriedHistory {
    CarriedHistory::new(
        turns
            .iter()
            .map(|(said, reply)| CarriedTurn::new(Some(said), Some(reply), None).unwrap())
            .collect(),
        0,
    )
    .unwrap()
}
fn fork(request: &str, turns: &[(&str, &str)]) -> HandoffRecord {
    HandoffRecord {
        id: HandoffId::of_request(&actor(request)),
        kind: HandoffKind::Fork {
            source: SessionId::new("source").unwrap(),
            history: history(turns),
        },
        actor: actor(request),
        at_ms: 3,
        after_execution: None,
    }
}
/// A finished turn `id` that said `said` and answered `replies` in order,
/// with a tool step before the last one.
fn answered(id: &str, said: &str, replies: &[&str]) -> InvocationRecord {
    let mut record = turn(
        id,
        UserMessage::text_only(PromptText::new(said).unwrap()),
        Vec::new(),
    );
    let finished = record.events.pop().unwrap();
    let execution = record.request.execution_id.clone();
    for (index, reply) in replies.iter().enumerate() {
        if index + 1 == replies.len() && index > 0 {
            record.events.push(ExecutionEvent::new(
                execution.clone(),
                ExecutionUpdate::Tool(tool_call("call")),
            ));
        }
        record.events.push(ExecutionEvent::new(
            execution.clone(),
            ExecutionUpdate::Message(MessageChunk::text(*reply)),
        ));
    }
    record.events.push(finished);
    record
}

#[test]
fn a_handoff_id_is_the_requests_own_and_bounded() {
    let id = HandoffId::of_request(&actor("one"));
    assert_eq!(id.as_str().len(), 32);
    assert_eq!(id, HandoffId::of_request(&actor("one")));
    assert_ne!(id, HandoffId::of_request(&actor("two")));
    assert_ne!(
        id,
        HandoffId::of_request(&ActionContext::new("other", "desktop", "one").unwrap())
    );
    assert_eq!(HandoffId::new(id.as_str()).unwrap(), id);
    let over = "x".repeat(HandoffId::MAX_BYTES + 1);
    for refused in ["", "  ", over.as_str()] {
        assert!(matches!(
            HandoffId::new(refused),
            Err(StorageError::Corrupt(_))
        ));
    }
}

#[test]
fn carried_history_keeps_the_newest_turns_that_fit() {
    assert!(CarriedTurn::new(None, Some(" "), None).is_err());
    let long = "x".repeat(CarriedTurn::MAX_PART_BYTES + 1);
    assert!(CarriedTurn::new(Some(&long), None, None).is_err());
    assert_eq!(
        CarriedTurn::clipped(Some(&long), None, None)
            .unwrap()
            .said()
            .unwrap()
            .len(),
        CarriedTurn::MAX_PART_BYTES
    );
    let full = "y".repeat(CarriedTurn::MAX_PART_BYTES);
    let turns: Vec<_> = (0..10)
        .map(|_| CarriedTurn::new(Some(&full), None, None).unwrap())
        .collect();
    let kept = CarriedHistory::newest(1, turns.clone());
    assert_eq!(
        kept.turns().len(),
        CarriedHistory::MAX_TEXT_BYTES / CarriedTurn::MAX_PART_BYTES
    );
    assert_eq!(kept.omitted(), 1 + 10 - kept.turns().len() as u32);
    assert!(CarriedHistory::new(turns, 0).is_err());
    assert!(CarriedHistory::default().is_empty());
}

/// A turn is given the text after its last tool step; a fork's turns come
/// first, then the conversation's own, before the turn being dispatched.
#[test]
fn the_history_before_a_turn_is_the_forks_copy_then_its_own_turns() {
    let mut saved = snapshot(vec![
        answered("one", "plot May", &["drawing", "done: may.png"]),
        answered("two", "and June", &["june.png"]),
        answered("three", "thanks", &["any time"]),
    ]);
    saved.handoffs = vec![fork("fork", &[("hello", "hi")])];
    let history = history_before(&saved, 2);
    let turns: Vec<_> = history
        .turns()
        .iter()
        .map(|turn| (turn.said().unwrap(), turn.reply().unwrap()))
        .collect();
    assert_eq!(
        turns,
        vec![
            ("hello", "hi"),
            ("plot May", "done: may.png"),
            ("and June", "june.png"),
        ]
    );
    assert_eq!(saved.carried_history().turns().len(), 4);
    assert!(snapshot(Vec::new()).carried_history().is_empty());
}

/// Rows H1–H4: a switch must leave the identity the history holds for
/// another name or model, after the last turn, once per request.
#[test]
fn a_switch_leaves_the_identity_the_history_holds_after_its_last_turn() {
    let saved = snapshot(vec![answered("one", "plot", &["done"])]);
    let to = identity("other", "model");
    assert_eq!(
        admit_saved(&saved, &switch("s", Some("one"), to.clone())),
        Ok(())
    );
    assert_eq!(
        admit_saved(
            &saved,
            &switch("s", Some("one"), identity("provider", "other-model"))
        ),
        Ok(())
    );
    // Context alone is not a switch.
    assert!(matches!(
        admit_saved(
            &saved,
            &switch(
                "s",
                Some("one"),
                ProviderIdentity::new("provider", "model", "ctx").unwrap()
            )
        ),
        Err(StorageError::Corrupt(_))
    ));
    let mut from_elsewhere = switch("s", Some("one"), to.clone());
    from_elsewhere.kind = HandoffKind::Switch {
        before: identity("third", "model"),
        after: to.clone(),
    };
    assert_eq!(
        admit_saved(&saved, &from_elsewhere),
        Err(StorageError::IdentityMismatch)
    );
    for misplaced in [None, Some("missing")] {
        assert!(matches!(
            admit_saved(&saved, &switch("s", misplaced, to.clone())),
            Err(StorageError::Corrupt(_))
        ));
    }
    let mut late = switch("s", Some("one"), to.clone());
    late.at_ms = HandoffRecord::MAX_AT_MS + 1;
    assert!(matches!(
        admit_saved(&saved, &late),
        Err(StorageError::Corrupt(_))
    ));
    let mut repeated = saved.clone();
    repeated.handoffs.push(switch("s", Some("one"), to.clone()));
    assert!(matches!(
        admit_saved(&repeated, &switch("s", Some("one"), to.clone())),
        Err(StorageError::Corrupt(_))
    ));
    assert!(repeated.handoff("person", "s").is_some());
    assert!(repeated.handoff("person", "t").is_none());
    let mut full = saved.clone();
    full.handoffs = (0..HandoffRecord::MAX_PER_CONVERSATION)
        .map(|index| switch(&format!("s{index}"), Some("one"), to.clone()))
        .collect();
    assert!(matches!(
        admit_saved(&full, &switch("new", Some("one"), to)),
        Err(StorageError::Corrupt(_))
    ));
}

/// Row H5: a fork is the first thing a new conversation holds, and never of
/// itself.
#[test]
fn a_fork_comes_first_and_never_from_itself() {
    let empty = snapshot(Vec::new());
    assert_eq!(admit_saved(&empty, &fork("f", &[("a", "b")])), Ok(()));
    let mut itself = fork("f", &[]);
    itself.kind = HandoffKind::Fork {
        source: empty.id.clone(),
        history: CarriedHistory::default(),
    };
    assert!(matches!(
        admit_saved(&empty, &itself),
        Err(StorageError::Corrupt(_))
    ));
    let mut forked = empty.clone();
    forked.handoffs.push(fork("f", &[]));
    assert!(matches!(
        admit_saved(&forked, &fork("g", &[])),
        Err(StorageError::Corrupt(_))
    ));
    let mut late = fork("f", &[]);
    late.after_execution = Some(ExecutionId::new("one").unwrap());
    assert!(matches!(
        admit_saved(&snapshot(vec![answered("one", "a", &["b"])]), &late),
        Err(StorageError::Corrupt(_))
    ));
}

/// Row H6: the first turn admitted after a handoff takes it, and no other.
#[test]
fn only_the_first_turn_after_the_latest_handoff_takes_it() {
    let mut saved = snapshot(vec![answered("one", "a", &["b"])]);
    assert_eq!(select(&saved), None);
    assert_eq!(admit_taken(&saved, None), Ok(()));
    let first = switch("s", Some("one"), identity("other", "model"));
    saved.handoffs.push(first.clone());
    assert_eq!(select(&saved), Some(first.id.clone()));
    assert_eq!(admit_taken(&saved, Some(&first.id)), Ok(()));
    let unknown = HandoffId::new("unknown").unwrap();
    assert!(matches!(
        admit_taken(&saved, Some(&unknown)),
        Err(StorageError::Corrupt(_))
    ));
    let mut taking = answered("two", "c", &["d"]);
    taking.handoff = Some(first.id.clone());
    saved.invocations.push(taking);
    assert_eq!(select(&saved), None);
    assert!(matches!(
        admit_taken(&saved, Some(&first.id)),
        Err(StorageError::Corrupt(_))
    ));
    assert_eq!(validate_saved(&saved), Ok(()));
}

#[test]
fn a_saved_handoff_out_of_place_or_taken_twice_is_corrupt() {
    let to = identity("other", "model");
    let mut taken = answered("two", "c", &["d"]);
    taken.handoff = Some(HandoffId::of_request(&actor("s")));
    let base = snapshot(vec![answered("one", "a", &["b"]), taken.clone()]);
    let mut valid = base.clone();
    valid.handoffs = vec![switch("s", Some("one"), to.clone())];
    assert_eq!(validate_saved(&valid), Ok(()));

    let mut cases = Vec::new();
    // Taken by the turn it follows.
    let mut same = base.clone();
    same.handoffs = vec![switch("s", Some("two"), to.clone())];
    cases.push(same);
    // Taken twice.
    let mut twice = valid.clone();
    twice.invocations.push(taken);
    cases.push(twice);
    // A turn takes a handoff that is not held.
    cases.push(base.clone());
    // After a turn the conversation does not hold.
    let mut missing = base.clone();
    missing.handoffs = vec![switch("s", Some("missing"), to.clone())];
    cases.push(missing);
    // Two with one request.
    let mut repeated = valid.clone();
    let mut again = switch("s", Some("one"), to.clone());
    again.id = HandoffId::new("other").unwrap();
    repeated.handoffs.push(again);
    cases.push(repeated);
    // A fork after a switch.
    let mut fork_later = valid.clone();
    fork_later.handoffs.push(fork("f", &[]));
    cases.push(fork_later);
    for case in cases {
        assert!(
            matches!(validate_saved(&case), Err(StorageError::Corrupt(_))),
            "{:?}",
            case.handoffs
        );
    }
}

/// The fold records a switch, moves the history to its identity and clears
/// the old provider's configuration; a unit that fails after it takes all of
/// that back.
#[test]
fn the_fold_records_a_switch_and_a_failed_unit_takes_it_back() {
    let saved = snapshot(vec![answered("one", "a", &["b"])]);
    let mut continuation = Continuation::empty();
    continuation.apply_unit(&record_log(&saved)).unwrap();
    let accounted = continuation.snapshot_bytes;
    let to = identity("other", "model");
    let refused = continuation.apply_unit(&[
        SessionChange::Handoff(switch("s", Some("one"), to.clone())),
        SessionChange::Handoff(switch("s", Some("one"), to.clone())),
    ]);
    assert!(matches!(refused, Err(StorageError::Corrupt(_))));
    let folded = continuation.snapshot.as_ref().unwrap();
    assert!(folded.handoffs.is_empty());
    assert_eq!(folded.provider, saved.provider);
    // The list keeps the room it grew, and that room is accounted.
    assert_eq!(continuation.snapshot_bytes, retained::snapshot(folded));

    continuation
        .apply_unit(&[SessionChange::Handoff(switch("s", Some("one"), to.clone()))])
        .unwrap();
    let folded = continuation.snapshot.as_ref().unwrap();
    assert_eq!(folded.provider, to);
    assert_eq!(folded.handoffs.len(), 1);
    assert_eq!(continuation.snapshot_bytes, retained::snapshot(folded));
    assert!(continuation.snapshot_bytes > accounted);

    // The next turn takes it.
    let mut next = answered("two", "c", &["d"]);
    next.events.clear();
    next.result = None;
    next.local_outcome = None;
    next.handoff = Some(folded.handoffs[0].id.clone());
    continuation
        .apply_unit(&[SessionChange::InputAccepted(Box::new(next.clone()))])
        .unwrap();
    let folded = continuation.snapshot.as_ref().unwrap();
    assert_eq!(continuation.snapshot_bytes, retained::snapshot(folded));
    next.request.execution_id = ExecutionId::new("three").unwrap();
    assert!(matches!(
        continuation.apply_unit(&[SessionChange::InputAccepted(Box::new(next))]),
        Err(StorageError::Corrupt(_))
    ));
}

#[test]
fn the_fold_records_a_fork_first() {
    let saved = snapshot(Vec::new());
    let mut continuation = Continuation::empty();
    continuation.apply_unit(&record_log(&saved)).unwrap();
    continuation
        .apply_unit(&[SessionChange::Handoff(fork("f", &[("a", "b")]))])
        .unwrap();
    let folded = continuation.snapshot.as_ref().unwrap();
    assert_eq!(folded.provider, saved.provider);
    assert_eq!(folded.carried_history().turns().len(), 1);
    assert_eq!(continuation.snapshot_bytes, retained::snapshot(folded));
}
