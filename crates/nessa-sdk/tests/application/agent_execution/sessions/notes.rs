//! Notes: the values one holds, the one rule a note is accepted and read back
//! under, and the fold that appends it and takes it back.

use super::super::test_support::{record_log, snapshot};
use super::*;
use crate::application::agent_execution::sessions::{
    records::continuation::Continuation, retained, SessionChange,
};
use crate::domain::agent_execution::executions::GrantInitiator;

fn actor(request: &str) -> ActionContext {
    ActionContext::new("person", "desktop", request).unwrap()
}
fn grant(revision: u64) -> Initiator {
    Initiator::Grant(GrantInitiator::new("receiver", revision).unwrap())
}
fn request(request: &str, text: &str) -> NoteRequest {
    NoteRequest {
        text: NoteText::new(text).unwrap(),
        delivery: NoteDelivery::RecordOnly,
        actor: actor(request),
        author: Initiator::Owner,
    }
}
fn record(id: &str, request: &str, accepted_at_ms: u64) -> NoteRecord {
    let NoteRequest {
        text,
        delivery,
        actor,
        author,
    } = self::request(request, "a note");
    NoteRecord {
        id: NoteId::new(id).unwrap(),
        text,
        delivery,
        actor,
        author,
        accepted_at_ms,
    }
}
/// Admit `request` into `snapshot` at `now` and keep it, as the manager does.
fn accept(snapshot: &mut SessionSnapshot, request: NoteRequest, now: u64) -> NoteRecord {
    match admit(snapshot, request, now).unwrap() {
        NoteAdmission::New(record) => {
            snapshot.notes.push(record.clone());
            record
        }
        NoteAdmission::Recorded(record) => panic!("already recorded: {record:?}"),
    }
}

#[test]
fn note_text_and_ids_are_bounded_and_never_blank() {
    let longest = "é".repeat(NoteText::MAX_BYTES / 2);
    for accepted in ["a note", " padded ", longest.as_str()] {
        assert_eq!(NoteText::new(accepted).unwrap().as_str(), accepted);
    }
    let over = "x".repeat(NoteText::MAX_BYTES + 1);
    for refused in ["", " \n\t", over.as_str()] {
        assert_eq!(NoteText::new(refused), Err(NoteValueError::Text));
    }
    let minted = NoteId::mint();
    assert_eq!(minted.as_str().len(), 36);
    assert_ne!(minted, NoteId::mint());
    assert_eq!(NoteId::new(minted.as_str()).unwrap(), minted);
    let over = "x".repeat(NoteId::MAX_BYTES + 1);
    for refused in ["", "  ", over.as_str()] {
        assert_eq!(NoteId::new(refused), Err(NoteValueError::Id));
    }
    assert_ne!(
        NoteValueError::Id.to_string(),
        NoteValueError::Text.to_string()
    );
}

/// Row J16: a new note takes a fresh id and the clock's time, keeps its
/// author and actor as admitted, and is recorded, never pending.
#[test]
fn a_new_note_takes_a_fresh_id_and_the_time_it_was_accepted() {
    let mut saved = snapshot(Vec::new());
    let mut from_grant = request("comment", "from the phone");
    from_grant.author = grant(7);
    let first = accept(&mut saved, request("note", "hello"), 1_000);
    let second = accept(&mut saved, from_grant.clone(), 2_000);
    assert_ne!(first.id, second.id);
    assert_eq!(first.accepted_at_ms, 1_000);
    assert_eq!(first.text.as_str(), "hello");
    assert_eq!(first.author, Initiator::Owner);
    assert_eq!(second.author, grant(7));
    assert_eq!(second.actor, from_grant.actor);
    assert_eq!(first.state(), NoteState::Recorded);
    assert_eq!(saved.note("person", "note"), Some(&first));
    assert_eq!(saved.note("person", "comment"), Some(&second));
    assert_eq!(saved.note("someone", "note"), None);
    assert_eq!(saved.note("person", "other"), None);
    // A clock past what a receipt can state holds at the latest it can.
    let late = accept(&mut saved, request("late", "late"), u64::MAX);
    assert_eq!(late.accepted_at_ms, NoteRecord::MAX_ACCEPTED_AT_MS);
}

/// Row J18: the same request again is answered by its record, unchanged;
/// another note under the same principal's request is a conflict, whatever
/// differs.
#[test]
fn the_same_request_is_answered_by_its_record_and_another_conflicts() {
    let mut saved = snapshot(Vec::new());
    let first = accept(&mut saved, request("note", "hello"), 1_000);
    assert_eq!(
        admit(&saved, request("note", "hello"), 9_000),
        Ok(NoteAdmission::Recorded(first))
    );
    let mut other_surface = request("note", "hello");
    other_surface.actor = ActionContext::new("person", "phone", "note").unwrap();
    let mut other_author = request("note", "hello");
    other_author.author = grant(1);
    for conflicting in [request("note", "changed"), other_surface, other_author] {
        assert_eq!(
            admit(&saved, conflicting, 9_000),
            Err(NoteRefusal::Conflict)
        );
    }
    // The same grant's retry after its policy changed is the same note, and
    // keeps the revision it was accepted under.
    let mut from_grant = request("grant-note", "hello");
    from_grant.author = grant(1);
    let granted = accept(&mut saved, from_grant.clone(), 1_000);
    from_grant.author = grant(2);
    assert_eq!(
        admit(&saved, from_grant.clone(), 9_000),
        Ok(NoteAdmission::Recorded(granted))
    );
    from_grant.author = Initiator::Grant(GrantInitiator::new("other", 1).unwrap());
    assert_eq!(admit(&saved, from_grant, 9_000), Err(NoteRefusal::Conflict));
    // Another principal's request of the same name is its own.
    let mut someone = request("note", "hello");
    someone.actor = ActionContext::new("someone", "desktop", "note").unwrap();
    assert!(matches!(
        admit(&saved, someone, 9_000),
        Ok(NoteAdmission::New(_))
    ));
}

/// Row J19: an author leaves at most its rate in any window; another author
/// is counted apart, and the window moving on makes room. A clock set back
/// makes none.
#[test]
fn an_author_leaves_at_most_its_rate_in_any_window() {
    let mut saved = snapshot(Vec::new());
    for index in 0..NoteRecord::MAX_PER_AUTHOR_IN_WINDOW {
        accept(&mut saved, request(&format!("note-{index}"), "x"), 0);
    }
    assert_eq!(
        admit(&saved, request("one-more", "x"), 0),
        Err(NoteRefusal::RateLimited)
    );
    assert_eq!(
        admit(
            &saved,
            request("one-more", "x"),
            NoteRecord::RATE_WINDOW_MS - 1
        ),
        Err(NoteRefusal::RateLimited)
    );
    let mut from_grant = request("one-more", "x");
    from_grant.author = grant(1);
    let mut granted = saved.clone();
    accept(&mut granted, from_grant.clone(), 0);
    assert!(matches!(
        admit(&saved, from_grant, 0),
        Ok(NoteAdmission::New(_))
    ));
    // The same grant at a later revision is the same author: a change of
    // policy gives no fresh rate.
    for index in 1..NoteRecord::MAX_PER_AUTHOR_IN_WINDOW {
        let mut next = request(&format!("grant-{index}"), "x");
        next.author = grant(1);
        accept(&mut granted, next, 0);
    }
    let mut bumped = request("bumped", "x");
    bumped.author = grant(2);
    assert_eq!(admit(&granted, bumped, 0), Err(NoteRefusal::RateLimited));
    assert!(matches!(
        admit(&saved, request("one-more", "x"), NoteRecord::RATE_WINDOW_MS),
        Ok(NoteAdmission::New(_))
    ));
    // Notes accepted at a later time than now still count.
    let mut later = snapshot(Vec::new());
    for index in 0..NoteRecord::MAX_PER_AUTHOR_IN_WINDOW {
        accept(&mut later, request(&format!("note-{index}"), "x"), 500_000);
    }
    assert_eq!(
        admit(&later, request("one-more", "x"), 1_000),
        Err(NoteRefusal::RateLimited)
    );
}

/// Row J19: the conversation keeps at most its bound, and a request already
/// recorded is still answered when it is full.
#[test]
fn a_full_conversation_refuses_a_new_note_and_still_answers_a_retry() {
    let mut saved = snapshot(Vec::new());
    saved.notes = (0..NoteRecord::MAX_PER_CONVERSATION)
        .map(|index| record(&format!("id-{index}"), &format!("note-{index}"), 0))
        .collect();
    assert_eq!(
        admit(&saved, request("one-more", "x"), u64::MAX),
        Err(NoteRefusal::Full)
    );
    assert_eq!(
        admit(&saved, request("note-3", "a note"), u64::MAX),
        Ok(NoteAdmission::Recorded(saved.notes[3].clone()))
    );
}

#[test]
fn a_saved_snapshot_keeps_one_note_per_id_and_request_within_its_bound() {
    let mut saved = snapshot(Vec::new());
    saved.notes = vec![record("a", "one", 0), record("b", "two", 0)];
    assert_eq!(validate_saved(&saved), Ok(()));
    for notes in [
        vec![record("a", "one", 0), record("a", "two", 0)],
        vec![record("a", "one", 0), record("b", "one", 0)],
        (0..=NoteRecord::MAX_PER_CONVERSATION)
            .map(|index| record(&format!("id-{index}"), &format!("note-{index}"), 0))
            .collect(),
        vec![record("a", "one", NoteRecord::MAX_ACCEPTED_AT_MS + 1)],
    ] {
        saved.notes = notes;
        assert!(matches!(
            validate_saved(&saved),
            Err(StorageError::Corrupt(_))
        ));
    }
}

/// The fold appends an admitted note and accounts its bytes; a unit that
/// fails after it takes the note and its bytes back with it.
#[test]
fn the_fold_appends_a_note_and_a_failed_unit_takes_it_back() {
    let saved = snapshot(Vec::new());
    let mut continuation = Continuation::empty();
    continuation.apply_unit(&record_log(&saved)).unwrap();
    let before = continuation.snapshot_bytes;

    let first = record("a", "one", 0);
    continuation
        .apply_unit(&[SessionChange::Note(first.clone())])
        .unwrap();
    let folded = continuation.snapshot.as_ref().unwrap();
    assert_eq!(folded.notes, vec![first.clone()]);
    assert_eq!(continuation.snapshot_bytes, retained::snapshot(folded));
    assert!(continuation.snapshot_bytes > before);
    let accounted = continuation.snapshot_bytes;

    // The second is admitted and then taken back when the one after it,
    // under the same request, is refused.
    let refused = continuation.apply_unit(&[
        SessionChange::Note(record("b", "two", 0)),
        SessionChange::Note(record("c", "two", 0)),
    ]);
    assert!(matches!(refused, Err(StorageError::Corrupt(_))));
    for repeated in [record("a", "three", 0), record("d", "one", 0)] {
        assert!(matches!(
            continuation.apply_unit(&[SessionChange::Note(repeated)]),
            Err(StorageError::Corrupt(_))
        ));
    }
    let folded = continuation.snapshot.as_ref().unwrap();
    assert_eq!(folded.notes, vec![first]);
    assert_eq!(continuation.snapshot_bytes, accounted);
}

#[test]
fn a_note_before_the_conversation_opened_or_past_its_bound_is_corrupt() {
    let mut continuation = Continuation::empty();
    assert!(matches!(
        continuation.apply_unit(&[SessionChange::Note(record("a", "one", 0))]),
        Err(StorageError::Corrupt(message)) if message.contains("precedes session open")
    ));
    let mut full = snapshot(Vec::new());
    full.notes = (0..NoteRecord::MAX_PER_CONVERSATION)
        .map(|index| record(&format!("id-{index}"), &format!("note-{index}"), 0))
        .collect();
    assert!(matches!(
        admit_saved(&full, &record("new", "new", 0)),
        Err(StorageError::Corrupt(_))
    ));
    let empty = snapshot(Vec::new());
    assert_eq!(
        admit_saved(&empty, &record("a", "one", NoteRecord::MAX_ACCEPTED_AT_MS)),
        Ok(())
    );
    assert!(matches!(
        admit_saved(
            &empty,
            &record("a", "one", NoteRecord::MAX_ACCEPTED_AT_MS + 1)
        ),
        Err(StorageError::Corrupt(_))
    ));
}
