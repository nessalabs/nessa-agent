//! Row J23: which submissions offer the agent the notes waiting for a turn.

use super::*;

#[test]
fn a_persons_new_queued_message_offers_notes_whether_or_not_a_turn_runs_now() {
    // Offered without asking whether the conversation is idle: the agent
    // decides that when it admits the turn.
    assert!(offers_notes(false, true, &SubmissionMode::Queue));
    // A retry was admitted already, an app or agent never offers notes, and
    // a steer joins a turn already running.
    assert!(!offers_notes(true, true, &SubmissionMode::Queue));
    assert!(!offers_notes(false, false, &SubmissionMode::Queue));
    assert!(!offers_notes(false, true, &SubmissionMode::Steer));
}
