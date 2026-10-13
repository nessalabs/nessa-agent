//! Notes: the values one holds, the one rule a note is accepted and read back
//! under, and the fold that appends it and takes it back.

use super::super::test_support::{record_log, snapshot};
use super::*;
use crate::application::agent_execution::sessions::{
    records::continuation::Continuation, retained, InvocationRecord, SessionChange,
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
        after_execution: None,
        retired: None,
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
    assert_eq!(saved.note_state(&first), NoteState::Recorded);
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

fn pending(id: &str, request: &str, accepted_at_ms: u64) -> NoteRecord {
    NoteRecord {
        delivery: NoteDelivery::NextTurn,
        ..record(id, request, accepted_at_ms)
    }
}
fn ids(values: &[&str]) -> Vec<NoteId> {
    values.iter().map(|id| NoteId::new(*id).unwrap()).collect()
}
fn candidates(authorized: &[&str], revoked: &[&str]) -> NoteCandidates {
    NoteCandidates {
        authorized: ids(authorized),
        revoked: ids(revoked),
    }
}
fn turn_taking(id: &str, notes: &[&str]) -> InvocationRecord {
    let mut record =
        super::super::test_support::turn(id, super::super::test_support::text(), Vec::new());
    record.notes = ids(notes);
    record
}

/// Row J22: a `next_turn` note is pending until a turn takes it, and then
/// assigned to that turn; a retired one is retired; a `record_only` one is
/// only recorded.
#[test]
fn a_notes_state_is_read_from_the_turns_and_retirements_saved() {
    let mut saved = snapshot(vec![turn_taking("first", &["taken"])]);
    saved.notes = vec![
        record("recorded", "one", 0),
        pending("taken", "two", 0),
        pending("waiting", "three", 0),
        NoteRecord {
            retired: Some(NoteRetirement {
                reason: NoteRetirementReason::Expired,
                at_ms: 9,
            }),
            ..pending("retired", "four", 0)
        },
    ];
    let states: Vec<_> = saved
        .notes
        .iter()
        .map(|note| saved.note_state(note))
        .collect();
    assert_eq!(
        states,
        vec![
            NoteState::Recorded,
            NoteState::Assigned(ExecutionId::new("first").unwrap()),
            NoteState::Pending,
            NoteState::Retired(NoteRetirement {
                reason: NoteRetirementReason::Expired,
                at_ms: 9,
            }),
        ]
    );
    assert_eq!(validate_saved(&saved), Ok(()));
}

/// Row J22: a note is placed after the last turn accepted before it.
#[test]
fn a_new_note_follows_the_last_turn_accepted_before_it() {
    let mut saved = snapshot(Vec::new());
    assert_eq!(
        accept(&mut saved, request("a", "x"), 0).after_execution,
        None
    );
    saved.invocations.push(turn_taking("first", &[]));
    saved.invocations.push(turn_taking("second", &[]));
    assert_eq!(
        accept(&mut saved, request("b", "x"), 0).after_execution,
        Some(ExecutionId::new("second").unwrap())
    );
}

/// Row J23: in saved order, the admission of a turn retires each pending
/// note past its lifetime and each the host says lost its grant, and takes
/// up to eight the host authorized; a note in neither list waits, and a
/// turn that takes no notes still retires.
#[test]
fn a_turn_retires_what_ended_and_takes_at_most_eight_authorized_notes_in_order() {
    let lifetime = NoteRecord::PENDING_LIFETIME_MS;
    let now = lifetime + 100;
    let mut saved = snapshot(vec![turn_taking("first", &["gone"])]);
    saved.notes = vec![
        record("recorded", "recorded", now),
        pending("gone", "gone", now),
        pending("old", "old", 100),
        pending("revoked", "revoked", now),
        pending("unchecked", "unchecked", now),
    ];
    saved
        .notes
        .extend((0..10).map(|index| pending(&format!("n{index}"), &format!("r{index}"), now)));
    let all: Vec<String> = (0..10).rev().map(|index| format!("n{index}")).collect();
    let mut authorized: Vec<&str> = all.iter().map(String::as_str).collect();
    // Authorized does not rescue an overdue note, a revoked one, or one
    // already taken or only recorded.
    authorized.extend(["old", "gone", "recorded"]);
    let candidates = candidates(&authorized, &["revoked", "old", "missing"]);

    let selection = select(&saved, &candidates, true, now);
    assert_eq!(
        selection.retired,
        vec![
            NoteRetiredRecord {
                id: NoteId::new("old").unwrap(),
                retirement: NoteRetirement {
                    reason: NoteRetirementReason::Expired,
                    at_ms: now,
                },
            },
            NoteRetiredRecord {
                id: NoteId::new("revoked").unwrap(),
                retirement: NoteRetirement {
                    reason: NoteRetirementReason::PermissionLost,
                    at_ms: now,
                },
            },
        ]
    );
    assert_eq!(
        selection.taken,
        ids(&["n0", "n1", "n2", "n3", "n4", "n5", "n6", "n7"])
    );
    assert_eq!(selection.taken.len(), NoteRecord::MAX_PER_TURN);

    let busy = select(&saved, &candidates, false, now);
    assert_eq!(busy.retired, selection.retired);
    assert_eq!(busy.taken, Vec::new());
    // The lifetime ends exactly at its length, not a millisecond later.
    assert!(
        select(&saved, &NoteCandidates::default(), false, lifetime + 99)
            .retired
            .is_empty()
    );
    assert_eq!(
        select(&saved, &NoteCandidates::default(), false, lifetime + 100)
            .retired
            .len(),
        1
    );
}

/// Row J22: a conversation keeps at most 64 pending notes; an overdue one,
/// which the next turn retires, takes no room, and a `record_only` note is
/// never refused for it.
#[test]
fn a_conversation_holds_at_most_sixty_four_notes_waiting_for_a_turn() {
    let lifetime = NoteRecord::PENDING_LIFETIME_MS;
    // Past the author's rate window, inside every note's lifetime.
    let now = lifetime + NoteRecord::RATE_WINDOW_MS;
    let mut saved = snapshot(Vec::new());
    saved.notes = (0..NoteRecord::MAX_PENDING)
        .map(|index| pending(&format!("p{index}"), &format!("r{index}"), lifetime))
        .collect();
    let next = NoteRequest {
        delivery: NoteDelivery::NextTurn,
        ..request("next", "x")
    };
    assert_eq!(admit(&saved, next.clone(), now), Err(NoteRefusal::Full));
    assert!(matches!(
        admit(&saved, request("recorded", "x"), now),
        Ok(NoteAdmission::New(_))
    ));
    saved.notes[0].accepted_at_ms = 0;
    assert!(matches!(
        admit(&saved, next.clone(), now),
        Ok(NoteAdmission::New(_))
    ));
    saved.notes[0].accepted_at_ms = lifetime;
    saved.notes[0].retired = Some(NoteRetirement {
        reason: NoteRetirementReason::PermissionLost,
        at_ms: 1,
    });
    assert!(matches!(
        admit(&saved, next, now),
        Ok(NoteAdmission::New(_))
    ));
}

/// The fold takes a retirement and a turn's notes only for pending
/// `next_turn` notes, and a failed unit gives them back.
#[test]
fn the_fold_retires_and_assigns_only_pending_notes_and_undoes_a_failed_unit() {
    let mut saved = snapshot(Vec::new());
    saved.notes = vec![
        pending("a", "one", 0),
        pending("b", "two", 0),
        record("c", "three", 0),
    ];
    let mut continuation = Continuation::empty();
    continuation.apply_unit(&record_log(&saved)).unwrap();
    for note in &saved.notes {
        continuation
            .apply_unit(&[SessionChange::Note(note.clone())])
            .unwrap();
    }
    let accounted = continuation.snapshot_bytes;
    let retire = |id: &str| {
        SessionChange::NoteRetired(NoteRetiredRecord {
            id: NoteId::new(id).unwrap(),
            retirement: NoteRetirement {
                reason: NoteRetirementReason::Expired,
                at_ms: 5,
            },
        })
    };
    let accepted = |id: &str, notes: &[&str]| {
        let mut record = turn_taking(id, notes);
        record.events.clear();
        record.result = None;
        record.local_outcome = None;
        SessionChange::InputAccepted(Box::new(record))
    };
    // A unit retiring one note and taking it too fails as a whole.
    for refused in [
        vec![retire("a"), accepted("t", &["a"])],
        vec![retire("c")],
        vec![retire("missing")],
        vec![retire("a"), retire("a")],
        vec![accepted("t", &["c"])],
        vec![accepted("t", &["a", "a"])],
        vec![accepted("t", &["missing"])],
        vec![accepted(
            "t",
            &["a", "a", "a", "a", "a", "a", "a", "a", "a"],
        )],
    ] {
        assert!(
            matches!(
                continuation.apply_unit(&refused),
                Err(StorageError::Corrupt(_))
            ),
            "{refused:?}"
        );
        let folded = continuation.snapshot.as_ref().unwrap();
        assert!(folded.notes.iter().all(|note| note.retired.is_none()));
        assert!(folded.invocations.is_empty());
        assert_eq!(continuation.snapshot_bytes, accounted);
    }
    continuation
        .apply_unit(&[retire("a"), accepted("t", &["b"])])
        .unwrap();
    let folded = continuation.snapshot.as_ref().unwrap();
    assert_eq!(
        folded.note_state(&folded.notes[0]),
        NoteState::Retired(NoteRetirement {
            reason: NoteRetirementReason::Expired,
            at_ms: 5,
        })
    );
    assert_eq!(
        folded.note_state(&folded.notes[1]),
        NoteState::Assigned(ExecutionId::new("t").unwrap())
    );
    assert_eq!(continuation.snapshot_bytes, retained::snapshot(folded));
    // Neither can happen to a note again.
    for refused in [
        vec![retire("a")],
        vec![retire("b")],
        vec![accepted("u", &["b"])],
    ] {
        assert!(matches!(
            continuation.apply_unit(&refused),
            Err(StorageError::Corrupt(_))
        ));
    }
}

/// A saved snapshot whose notes break the rule of turns is corrupt.
#[test]
fn a_saved_note_taken_twice_retired_and_taken_or_following_no_turn_is_corrupt() {
    let retired = Some(NoteRetirement {
        reason: NoteRetirementReason::Expired,
        at_ms: 1,
    });
    let over: Vec<String> = (0..=NoteRecord::MAX_PER_TURN)
        .map(|index| format!("n{index}"))
        .collect();
    let over: Vec<&str> = over.iter().map(String::as_str).collect();
    let cases: Vec<(Vec<InvocationRecord>, Vec<NoteRecord>)> = vec![
        (
            vec![turn_taking("t", &["a"]), turn_taking("u", &["a"])],
            vec![pending("a", "one", 0)],
        ),
        (
            vec![turn_taking("t", &["a"])],
            vec![NoteRecord {
                retired,
                ..pending("a", "one", 0)
            }],
        ),
        (vec![turn_taking("t", &["a"])], vec![record("a", "one", 0)]),
        (vec![turn_taking("t", &["missing"])], Vec::new()),
        (
            vec![turn_taking("t", &over)],
            over.iter().map(|id| pending(id, id, 0)).collect(),
        ),
        (
            Vec::new(),
            vec![NoteRecord {
                retired,
                ..record("a", "one", 0)
            }],
        ),
        (
            Vec::new(),
            vec![NoteRecord {
                after_execution: Some(ExecutionId::new("missing").unwrap()),
                ..pending("a", "one", 0)
            }],
        ),
    ];
    for (invocations, notes) in cases {
        let mut saved = snapshot(invocations);
        saved.notes = notes;
        assert!(
            matches!(validate_saved(&saved), Err(StorageError::Corrupt(_))),
            "{saved:?}"
        );
    }
    // A note saved by the stream is never already retired, and follows a
    // turn the conversation holds.
    let empty = snapshot(Vec::new());
    for refused in [
        NoteRecord {
            retired,
            ..pending("a", "one", 0)
        },
        NoteRecord {
            after_execution: Some(ExecutionId::new("missing").unwrap()),
            ..pending("a", "one", 0)
        },
    ] {
        assert!(matches!(
            admit_saved(&empty, &refused),
            Err(StorageError::Corrupt(_))
        ));
    }
}

/// A note is given to the model with its author's kind, its surface and its
/// text.
#[test]
fn a_saved_note_is_given_to_the_model_with_its_author_and_surface() {
    let owner = record("a", "one", 0).context().unwrap();
    assert_eq!(
        (owner.author(), owner.surface(), owner.text()),
        (NoteAuthor::Owner, "desktop", "a note")
    );
    let from_grant = NoteRecord {
        author: grant(3),
        ..record("b", "two", 0)
    };
    assert_eq!(from_grant.context().unwrap().author(), NoteAuthor::Grant);
    // Every saved note fits what the model is given: the same bounds.
    assert_eq!(NoteContext::MAX_TEXT_BYTES, NoteText::MAX_BYTES);
    assert_eq!(
        NoteContext::MAX_SURFACE_BYTES,
        ActionContext::MAX_IDENTITY_BYTES
    );
}
