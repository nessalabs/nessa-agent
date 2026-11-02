//! Controlled orderings for a shared SQLite departure and its receipt.
use super::*;
use std::panic::{catch_unwind, panic_any, resume_unwind};
use std::sync::PoisonError;

#[test]
fn a_lost_departure_does_not_send_again() {
    for order in [DepartureOrder::WinnerFirst, DepartureOrder::LoserFirst] {
        overlapping_drafts(order, false);
    }
}

#[test]
fn a_failed_lease_wait_returns_the_durable_departure() {
    for order in [DepartureOrder::WinnerFirst, DepartureOrder::LoserFirst] {
        overlapping_drafts(order, true);
    }
}

fn overlapping_drafts(order: DepartureOrder, fail_waiter: bool) {
    overlapping_drafts_with_preparation(order, fail_waiter, || {}, || {});
}

#[test]
fn opening_sender_stores_does_not_spend_the_departure_entry_window() {
    overlapping_drafts_with_preparation(
        DepartureOrder::WinnerFirst,
        true,
        || thread::sleep(Duration::from_secs(3)),
        || thread::sleep(Duration::from_secs(3)),
    );
}

fn overlapping_drafts_with_preparation(
    order: DepartureOrder,
    fail_waiter: bool,
    prepare: fn(),
    draft_read: fn(),
) {
    let dir = directory();
    let path = path_in(&dir);
    person_hold(
        &mut opened(&path, Scripted::new(Vec::new(), Vec::new())),
        text_intent("command-1", "hello"),
    );
    // Open before a sender holds the lease: opening validates under a writer lock.
    let mut observer = SqliteIntents::open(&path).unwrap();
    let gate = Arc::new((
        Mutex::new(DepartureGate::new(order, 2, fail_waiter)),
        Condvar::new(),
    ));
    let sends = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new((Mutex::new(false), Condvar::new()));
    let release = ReleaseSend::new();
    let (done_tx, done_rx) = mpsc::channel();
    // Opening validates under a writer lock. Prepare both stores before either
    // concurrent send starts, outside the send-entry observation window.
    let prepared = || {
        prepare();
        let mut store = YieldingLease::open(&path, Arc::clone(&gate));
        store.draft_read = draft_read;
        store
    };
    let [first_store, second_store] = [prepared(), prepared()];
    let spawn = |store| {
        let sends = Arc::clone(&sends);
        let entered = Arc::clone(&entered);
        let release = Arc::clone(&release.0);
        let done_tx = done_tx.clone();
        thread::spawn(move || {
            let mut outbox = PhoneOutbox::new(
                store,
                PausingSend {
                    sends,
                    entered,
                    release,
                },
                SeqIds::default(),
                clock(),
            );
            let result = outbox
                .send(&command("command-1"))
                .map(|view| view.outcome());
            done_tx.send((thread::current().id(), result)).unwrap();
        })
    };
    let first = spawn(first_store);
    let second = spawn(second_store);
    if let Err(failure) = catch_unwind(|| wait_for_flag(&entered)) {
        let phase = format!("{:?}", gate.0.lock().unwrap());
        release.release();
        let first = first.join();
        let second = second.join();
        eprintln!(
            "{order:?}: entry failed at {phase}; joined={:?}; outcomes={:?}; final={:?}",
            (first.is_ok(), second.is_ok()),
            done_rx.try_iter().collect::<Vec<_>>(),
            gate.0.lock().unwrap()
        );
        resume_unwind(failure);
    }
    let (lock, cv) = &*gate;
    let state = wait_for(cv, lock.lock().unwrap(), |state| state.attempts.len() == 2);
    assert_eq!(state.drafts.len(), 2, "{state:?}");
    assert_eq!(state.departures.len(), 2, "{state:?}");
    assert_eq!(
        state
            .departures
            .iter()
            .filter(|(_, result)| result.is_ok())
            .count(),
        1
    );
    assert_eq!(
        state
            .departures
            .iter()
            .filter(|(_, result)| *result == Err(StoreError::Conflict))
            .count(),
        1
    );
    let sender = state
        .results
        .iter()
        .find(|(_, result)| result.is_ok())
        .unwrap()
        .0;
    assert_eq!(
        Some(sender) == state.winner,
        matches!(order, DepartureOrder::WinnerFirst)
    );
    drop(state);
    assert_departed(&mut observer);
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    if fail_waiter {
        let (waiter, outcome) = done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_ne!(waiter, sender);
        assert_eq!(
            outcome,
            Ok(PhoneOutcome::Unknown(UnknownCause::NotAdmitted))
        );
        let state = lock.lock().unwrap();
        assert!(
            state
                .results
                .contains(&(waiter, Err(StoreError::Unavailable))),
            "{state:?}"
        );
        eprintln!(
            "{order:?}: waiter={waiter:?}, outcome={outcome:?}, leases={:?}",
            state.results
        );
    }
    // The physical sender (and, with a live wait, the contender) must stay pending.
    let early = done_rx.recv_timeout(Duration::from_millis(400));
    assert_eq!(
        early,
        Err(mpsc::RecvTimeoutError::Timeout),
        "{order:?}, state={:?}",
        lock.lock().unwrap()
    );
    release.release();
    let expected = if fail_waiter { 1 } else { 2 };
    for _ in 0..expected {
        let (_, result) = done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(result, Ok(PhoneOutcome::Accepted));
    }
    first.join().unwrap();
    second.join().unwrap();
    let state = lock.lock().unwrap();
    assert_eq!(state.leases, if fail_waiter { 1 } else { 2 }, "{state:?}");
    drop(state);
    assert_accepted(&mut observer);
    // A caller that returned unknown can subsequently read the durable receipt.
    let mut later = opened(&path, Scripted::new(Vec::new(), Vec::new()));
    assert_eq!(
        later.send(&command("command-1")).unwrap().outcome(),
        PhoneOutcome::Accepted
    );
    assert_eq!(sends.load(Ordering::SeqCst), 1);
}

#[test]
fn a_sender_first_reading_departure_can_return_before_the_send() {
    let dir = directory();
    let path = path_in(&dir);
    person_hold(
        &mut opened(&path, Scripted::new(Vec::new(), Vec::new())),
        text_intent("command-1", "hello"),
    );
    let gate = Arc::new((
        Mutex::new(DepartureGate::new(DepartureOrder::WinnerFirst, 1, false)),
        Condvar::new(),
    ));
    // Both stores are opened before physical send. The second caller's initial
    // load happens only after entry into the gateway, so it cannot see Draft.
    let store = YieldingLease::open(&path, Arc::clone(&gate));
    let later_store = YieldingLease::open(&path, Arc::clone(&gate));
    let mut observer = SqliteIntents::open(&path).unwrap();
    let sends = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new((Mutex::new(false), Condvar::new()));
    let release = ReleaseSend::new();
    let mut sender = PhoneOutbox::new(
        store,
        PausingSend {
            sends: Arc::clone(&sends),
            entered: Arc::clone(&entered),
            release: Arc::clone(&release.0),
        },
        SeqIds::default(),
        clock(),
    );
    let first = thread::spawn(move || {
        sender
            .send(&command("command-1"))
            .map(|view| view.outcome())
    });
    wait_for_flag(&entered);
    assert_departed(&mut observer);
    let gateway = Scripted::new(Vec::new(), vec![LookupAnswer::NotFound]);
    let mut later = PhoneOutbox::new(later_store, gateway.clone(), SeqIds::default(), clock());
    let (done_tx, done_rx) = mpsc::channel();
    let second = thread::spawn(move || {
        let result = later.send(&command("command-1")).map(|view| view.outcome());
        done_tx.send(result).unwrap();
    });
    let outcome = done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        outcome,
        Ok(PhoneOutcome::Unknown(UnknownCause::NotAdmitted))
    );
    second.join().unwrap();
    assert_eq!(gateway.body().order, vec!["lookup"]);
    assert_eq!(gate.0.lock().unwrap().attempts.len(), 1);
    eprintln!("late initial read: {outcome:?}; one acquired lease, no competing lease attempt");
    assert_departed(&mut observer);
    release.release();
    assert_eq!(first.join().unwrap(), Ok(PhoneOutcome::Accepted));
    let mut later = opened(&path, Scripted::new(Vec::new(), Vec::new()));
    assert_eq!(
        later.send(&command("command-1")).unwrap().outcome(),
        PhoneOutcome::Accepted
    );
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    assert_accepted(&mut observer);
}

fn assert_departed(store: &mut SqliteIntents) {
    let row = store.load(&command("command-1")).unwrap().unwrap();
    assert!(row.departed());
    assert_eq!(
        row.outcome(),
        PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
    );
    assert_eq!(row.sends(), 0);
    assert_eq!(
        store
            .events("command-1")
            .unwrap()
            .iter()
            .map(|event| (event.cause, event.initiator))
            .collect::<Vec<_>>(),
        vec![
            (Cause::Held, Initiator::Person),
            (Cause::Departed, Initiator::Person)
        ]
    );
}

fn assert_accepted(store: &mut SqliteIntents) {
    let row = store.load(&command("command-1")).unwrap().unwrap();
    assert!(row.departed());
    assert_eq!(row.intent(), &text_intent("command-1", "hello"));
    assert_eq!(row.outcome(), PhoneOutcome::Accepted);
    assert_eq!(row.sends(), 1);
    assert_eq!(
        store
            .events("command-1")
            .unwrap()
            .iter()
            .map(|event| (event.cause, event.initiator))
            .collect::<Vec<_>>(),
        vec![
            (Cause::Held, Initiator::Person),
            (Cause::Departed, Initiator::Person),
            (Cause::SendAnswered, Initiator::Person)
        ]
    );
}

/// The sender and ordering failures stay distinct through cleanup.
struct SenderTimeout;
struct OrderingFailure;

#[test]
fn a_poisoned_sender_release_preserves_the_ordering_failure() {
    let released = Arc::new((Mutex::new(false), Condvar::new()));
    let sender_release = Arc::clone(&released);
    let sender = thread::spawn(move || {
        // PausingSend's deadline assertion can panic while holding this guard.
        let _guard = sender_release.0.lock().unwrap();
        panic_any(SenderTimeout);
    });
    assert!(sender.join().unwrap_err().is::<SenderTimeout>());
    assert!(released.0.is_poisoned());

    let failure = catch_unwind(|| {
        let _release = ReleaseSend(Arc::clone(&released));
        panic_any(OrderingFailure);
    });
    assert!(failure.unwrap_err().is::<OrderingFailure>());
    // Cleanup releases the flag while retaining the sender's poison evidence.
    assert!(*released.0.lock().unwrap_err().into_inner());
}

#[test]
fn an_entry_timeout_does_not_poison_a_later_physical_sender() {
    let entered = Arc::new((Mutex::new(false), Condvar::new()));
    let failure = catch_unwind(|| wait_for_flag(&entered));
    assert!(failure.is_err(), "a missing entry still fails its deadline");
    assert!(!entered.0.is_poisoned());
    let sends = Arc::new(AtomicUsize::new(0));
    let release = ReleaseSend::new();
    release.release();
    let mut gateway = PausingSend {
        sends: Arc::clone(&sends),
        entered: Arc::clone(&entered),
        release: Arc::clone(&release.0),
    };
    assert!(matches!(
        gateway.send(&text_intent("late", "hello")),
        SendAnswer::Receipt(Delivery::Queued)
    ));
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    assert!(*entered.0.lock().unwrap());
}

/// Release even when a main-thread assertion fails, so the physical owner can exit.
struct ReleaseSend(Arc<(Mutex<bool>, Condvar)>);
impl ReleaseSend {
    fn new() -> Self {
        Self(Arc::new((Mutex::new(false), Condvar::new())))
    }
    fn release(&self) {
        // A sender timeout may poison this flag. Cleanup must still release
        // it without replacing the main thread's ordering failure.
        *self.0 .0.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.0 .1.notify_all();
    }
}
impl Drop for ReleaseSend {
    fn drop(&mut self) {
        self.release();
    }
}

fn wait_for_flag(flag: &(Mutex<bool>, Condvar)) {
    let (lock, cv) = flag;
    let (value, _) = cv
        .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(5), |value| {
            !*value
        })
        .unwrap();
    let entered = *value;
    drop(value);
    assert!(entered, "gateway did not enter or release its send");
}
