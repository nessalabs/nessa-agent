//! Actual SQLite lock refusal, independent of the substituted application wait.
use super::*;
use nessa_sdk::domain::agent_execution::executions::ExecutionId;
use std::sync::mpsc;
use std::thread;

#[test]
fn a_bounded_sqlite_lease_wait_keeps_the_committed_departure() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("outbox");
    nessa_local_storage::create_directory(&private).unwrap();
    let path = private.join("outbox.sqlite");
    let mut owner = SqliteIntents::open(&path).unwrap();
    let mut contender = SqliteIntents::open(&path).unwrap();
    let configured: u32 = contender
        .connection
        .pragma_query_value(None, "busy_timeout", |row| row.get(0))
        .unwrap();
    assert_eq!(u128::from(configured), LEASE_WAIT.as_millis());
    // Configure only this synthetic connection; exercise SQLite's own busy
    // handling without spending the production 30-second bound in every suite.
    contender
        .connection
        .busy_timeout(Duration::from_millis(10))
        .unwrap();
    let command = PhoneCommandId::new("bounded-wait").unwrap();
    let intent = PhoneIntent::new(
        "01234567-89ab-4cde-8f01-23456789abcd",
        command.clone(),
        ExecutionId::new("bounded-wait").unwrap(),
        "synthetic",
        Vec::new(),
    )
    .unwrap();
    let draft = PhoneRecord::held(intent);
    let departed = draft.clone().depart().unwrap();
    let event = |cause, outcome| IntentEvent {
        cause,
        outcome,
        initiator: Initiator::Person,
        observed_at_ms: 1,
        successor: None,
    };
    owner
        .insert(&draft, event(Cause::Held, draft.outcome()))
        .unwrap();
    owner
        .replace(
            &draft,
            &departed,
            event(Cause::Departed, departed.outcome()),
        )
        .unwrap();
    owner.begin_exclusive().unwrap();
    let (done_tx, done_rx) = mpsc::channel();
    let waiter = thread::spawn(move || {
        let result = contender.begin_exclusive();
        assert!(done_tx.send((result, contender)).is_ok());
    });
    let (result, mut contender) = done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("bounded SQLite wait did not return");
    waiter.join().unwrap();
    assert_eq!(result, Err(StoreError::Unavailable));
    assert!(!contender.leased);
    assert!(contender.connection.is_autocommit());
    assert_eq!(contender.load(&command).unwrap(), Some(departed.clone()));
    assert_eq!(contender.events(command.as_str()).unwrap().len(), 2);
    owner.abort_exclusive();
    contender.begin_exclusive().unwrap();
    assert_eq!(contender.load(&command).unwrap(), Some(departed));
    contender.end_exclusive().unwrap();
}
