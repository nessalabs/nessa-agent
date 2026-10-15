//! The phone outbox against a real SQLite file and a scripted gateway.
use super::sqlite::{open, SqliteIntents};
use crate::outbox::application::StoreError;
use crate::outbox::application::{CommandIds, IntentStore, PhoneOutbox};
use crate::outbox::domain::{
    history_reaches, Cause, Delivery, Initiator, IntentError, IntentEvent, LookupAnswer,
    PhoneCommandId, PhoneFailure, PhoneImage, PhoneIntent, PhoneOutcome, PhoneRecord,
    ReceiptEffect, ReceiptStage, SendAnswer, UnknownCause, FOREGROUND_SENDS,
};
use crate::outbox::{OutboxError, PhoneGateway, PhoneView};
use nessa_auth::application::ports::Clock;
use nessa_local_database::rusqlite::{params, Connection};
use nessa_protocol::product_contract::generated::ConversationErrorCode;
use nessa_sdk::domain::agent_execution::executions::ExecutionId;
use nessa_sdk::domain::common::value_objects::{ImageMediaType, Sha256Digest};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier, Condvar, Mutex};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

const CONVERSATION: &str = "01234567-89ab-4cde-8f01-23456789abcd";

struct FixedClock;
impl Clock for FixedClock {
    fn unix_milliseconds(&self) -> u64 {
        1_700_000_000_000
    }
}

#[derive(Default)]
struct SeqIds {
    next: u64,
}
impl CommandIds for SeqIds {
    fn mint(&mut self) -> Result<(PhoneCommandId, ExecutionId), OutboxError> {
        self.next += 1;
        let command = PhoneCommandId::new(format!("cmd-{}", self.next)).map_err(OutboxError::Id)?;
        let turn = ExecutionId::new(format!("turn-new-{}", self.next))
            .map_err(|_| OutboxError::Id(IntentError::EmptyId))?;
        Ok((command, turn))
    }
}

struct Body {
    sends: VecDeque<SendAnswer>,
    lookups: VecDeque<LookupAnswer>,
    sent: Vec<Vec<u8>>,
    looked: Vec<Vec<u8>>,
    order: Vec<&'static str>,
}
#[derive(Clone)]
struct Scripted(Arc<Mutex<Body>>);
impl Scripted {
    fn new(sends: Vec<SendAnswer>, lookups: Vec<LookupAnswer>) -> Self {
        Self(Arc::new(Mutex::new(Body {
            sends: VecDeque::from(sends),
            lookups: VecDeque::from(lookups),
            sent: Vec::new(),
            looked: Vec::new(),
            order: Vec::new(),
        })))
    }
    fn body(&self) -> std::sync::MutexGuard<'_, Body> {
        self.0.lock().expect("script")
    }
}
impl PhoneGateway for Scripted {
    fn send(&mut self, intent: &PhoneIntent) -> SendAnswer {
        let mut body = self.body();
        body.order.push("send");
        body.sent.push(intent.canonical().to_vec());
        body.sends
            .pop_front()
            .expect("unexpected conversation.send")
    }
    fn lookup(&mut self, intent: &PhoneIntent) -> LookupAnswer {
        let mut body = self.body();
        body.order.push("lookup");
        body.looked.push(intent.canonical().to_vec());
        body.lookups
            .pop_front()
            .expect("unexpected conversation.receipt")
    }
}

struct Memory {
    rows: BTreeMap<String, PhoneRecord>,
    events: BTreeMap<String, Vec<IntentEvent>>,
    uncertain_depart: bool,
    fail_answer: Option<StoreError>,
}
impl IntentStore for Memory {
    fn load(&mut self, id: &PhoneCommandId) -> Result<Option<PhoneRecord>, StoreError> {
        Ok(self.rows.get(id.as_str()).cloned())
    }
    fn insert(&mut self, record: &PhoneRecord, event: IntentEvent) -> Result<(), StoreError> {
        let id = record.intent().command().as_str().to_owned();
        self.events.entry(id.clone()).or_default().push(event);
        self.rows.insert(id, record.clone());
        Ok(())
    }
    fn replace(
        &mut self,
        previous: &PhoneRecord,
        next: &PhoneRecord,
        event: IntentEvent,
    ) -> Result<(), StoreError> {
        if self.uncertain_depart && !previous.departed() && next.departed() {
            return Err(StoreError::Uncertain);
        }
        if let Some(error) = self.fail_answer {
            if previous.departed() && next.outcome() != previous.outcome() {
                return Err(error);
            }
        }
        let id = next.intent().command().as_str().to_owned();
        self.events.entry(id.clone()).or_default().push(event);
        self.rows.insert(id, next.clone());
        Ok(())
    }
    fn replace_and_insert(
        &mut self,
        previous: &PhoneRecord,
        next: &PhoneRecord,
        draft: &PhoneRecord,
        events: &[IntentEvent],
        draft_event: IntentEvent,
    ) -> Result<(), StoreError> {
        let id = next.intent().command().as_str().to_owned();
        self.events
            .entry(id.clone())
            .or_default()
            .extend(events.iter().copied());
        self.rows.insert(id, next.clone());
        self.insert(draft, draft_event)?;
        let _ = previous;
        Ok(())
    }
    fn list(&mut self) -> Result<Vec<PhoneRecord>, StoreError> {
        Ok(self.rows.values().cloned().collect())
    }
    fn events(&mut self, id: &PhoneCommandId) -> Result<Vec<IntentEvent>, StoreError> {
        Ok(self.events.get(id.as_str()).cloned().unwrap_or_default())
    }
    fn begin_exclusive(&mut self) -> Result<(), StoreError> {
        Ok(())
    }
    fn end_exclusive(&mut self) -> Result<(), StoreError> {
        Ok(())
    }
    fn abort_exclusive(&mut self) {}
}

fn command(value: &str) -> PhoneCommandId {
    PhoneCommandId::new(value).unwrap()
}

fn turn(value: &str) -> ExecutionId {
    ExecutionId::new(value).unwrap()
}

fn text_intent(id: &str, text: &str) -> PhoneIntent {
    PhoneIntent::new(CONVERSATION, command(id), turn(id), text, Vec::new()).unwrap()
}

fn image() -> PhoneImage {
    PhoneImage::new(
        Sha256Digest::parse(
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap(),
        ImageMediaType::parse("image/png").unwrap(),
        4,
    )
    .unwrap()
}

fn directory() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn path_in(dir: &tempfile::TempDir) -> PathBuf {
    let private = dir.path().join("outbox");
    nessa_local_storage::create_directory(&private).unwrap();
    private.join("outbox.sqlite")
}

fn clock() -> Arc<dyn Clock> {
    Arc::new(FixedClock)
}

fn opened(path: &Path, gateway: Scripted) -> PhoneOutbox<SqliteIntents, Scripted, SeqIds> {
    open(path, gateway, SeqIds::default(), clock()).unwrap()
}

fn person_hold<S, G, I>(outbox: &mut PhoneOutbox<S, G, I>, intent: PhoneIntent) -> PhoneView
where
    S: IntentStore,
    G: PhoneGateway,
    I: CommandIds,
{
    outbox.hold(intent).unwrap()
}

fn depart(path: &Path, intent: PhoneIntent) {
    let mut store = SqliteIntents::open(path).unwrap();
    let draft = PhoneRecord::held(intent);
    store
        .insert(
            &draft,
            IntentEvent {
                cause: Cause::Held,
                outcome: PhoneOutcome::Draft,
                initiator: Initiator::Person,
                observed_at_ms: 1,
            },
        )
        .unwrap();
    let record = draft.clone().depart().unwrap();
    store
        .replace(
            &draft,
            &record,
            IntentEvent {
                cause: Cause::Departed,
                outcome: record.outcome(),
                initiator: Initiator::Person,
                observed_at_ms: 1,
            },
        )
        .unwrap();
}

fn failure(code: ConversationErrorCode) -> SendAnswer {
    SendAnswer::Refused(PhoneFailure::Conversation(code))
}

fn failure_lookup(code: ConversationErrorCode) -> LookupAnswer {
    LookupAnswer::Refused(PhoneFailure::Conversation(code))
}

fn settled(effect: ReceiptEffect) -> LookupAnswer {
    LookupAnswer::Found {
        stage: ReceiptStage::Settled,
        effect: Some(effect),
    }
}

#[test]
fn a_saved_draft_is_not_a_submission() {
    let dir = directory();
    let gateway = Scripted::new(Vec::new(), Vec::new());
    let seen = gateway.clone();
    let mut outbox = opened(&path_in(&dir), gateway);
    let view = person_hold(&mut outbox, text_intent("command-1", "hello"));
    assert_eq!(view.outcome(), PhoneOutcome::Draft);
    assert!(!view.claims_submission());
    assert_eq!(view.intent().text(), "hello");
    assert!(seen.body().order.is_empty());
}

#[test]
fn a_draft_survives_reopen_without_a_send() {
    let dir = directory();
    let path = path_in(&dir);
    {
        let gateway = Scripted::new(Vec::new(), Vec::new());
        let mut outbox = opened(&path, gateway);
        person_hold(&mut outbox, text_intent("command-1", "  hello  "));
    }
    let gateway = Scripted::new(Vec::new(), Vec::new());
    let seen = gateway.clone();
    let mut outbox = opened(&path, gateway);
    let views = outbox.list().unwrap();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].outcome(), PhoneOutcome::Draft);
    assert_eq!(views[0].intent().text(), "  hello  ");
    assert!(!views[0].claims_submission());
    assert!(seen.body().order.is_empty());
}

#[test]
fn a_changed_body_under_the_same_command_is_refused() {
    let dir = directory();
    let gateway = Scripted::new(Vec::new(), Vec::new());
    let seen = gateway.clone();
    let mut outbox = opened(&path_in(&dir), gateway);
    person_hold(&mut outbox, text_intent("command-1", "hello"));
    let changed = outbox.hold(text_intent("command-1", "hello!"));
    assert_eq!(changed.unwrap_err(), OutboxError::Conflict);
    let with_image = PhoneIntent::new(
        CONVERSATION,
        command("command-1"),
        turn("command-1"),
        "hello",
        vec![image()],
    )
    .unwrap();
    assert_eq!(outbox.hold(with_image).unwrap_err(), OutboxError::Conflict);
    let views = outbox.list().unwrap();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].intent().text(), "hello");
    assert!(views[0].intent().images().is_empty());
    assert!(seen.body().order.is_empty());
}

#[test]
fn saving_the_same_bytes_again_keeps_one_draft() {
    let dir = directory();
    let gateway = Scripted::new(Vec::new(), Vec::new());
    let mut outbox = opened(&path_in(&dir), gateway);
    let first = text_intent("command-1", "hello");
    let second = text_intent("command-1", "hello");
    person_hold(&mut outbox, first);
    let again = person_hold(&mut outbox, second);
    assert_eq!(again.outcome(), PhoneOutcome::Draft);
    assert_eq!(outbox.list().unwrap().len(), 1);
}

#[test]
fn departure_is_durable_before_the_gateway_is_asked() {
    let dir = directory();
    let path = path_in(&dir);
    let departed = Arc::new(Mutex::new(None));
    struct Watch {
        path: PathBuf,
        departed: Arc<Mutex<Option<i64>>>,
    }
    impl PhoneGateway for Watch {
        fn send(&mut self, intent: &PhoneIntent) -> SendAnswer {
            let connection = Connection::open(&self.path).unwrap();
            let bit: i64 = connection
                .query_row(
                    "SELECT departed FROM phone_intent WHERE request_id = ?1",
                    params![intent.command().as_str()],
                    |row| row.get(0),
                )
                .unwrap();
            *self.departed.lock().unwrap() = Some(bit);
            SendAnswer::Receipt(Delivery::Queued)
        }
        fn lookup(&mut self, _: &PhoneIntent) -> LookupAnswer {
            panic!("receipt lookup was not part of this send");
        }
    }
    let mut outbox = open(
        &path,
        Watch {
            path: path.clone(),
            departed: Arc::clone(&departed),
        },
        SeqIds::default(),
        clock(),
    )
    .unwrap();
    person_hold(&mut outbox, text_intent("command-1", "hello"));
    let view = outbox.send(&command("command-1")).unwrap();
    assert_eq!(*departed.lock().unwrap(), Some(1));
    assert_eq!(view.outcome(), PhoneOutcome::Accepted);
    assert!(view.claims_submission());
}

#[test]
fn an_uncertain_departure_does_not_send() {
    let gateway = Scripted::new(vec![SendAnswer::Receipt(Delivery::Queued)], Vec::new());
    let seen = gateway.clone();
    let mut outbox = PhoneOutbox::new(
        Memory {
            rows: BTreeMap::new(),
            events: BTreeMap::new(),
            uncertain_depart: true,
            fail_answer: None,
        },
        gateway,
        SeqIds::default(),
        clock(),
    );
    person_hold(&mut outbox, text_intent("command-1", "hello"));
    let error = outbox.send(&command("command-1"));
    assert_eq!(error.unwrap_err(), OutboxError::Uncertain);
    assert!(seen.body().order.is_empty());
    let still = outbox.list().unwrap();
    assert_eq!(still[0].outcome(), PhoneOutcome::Draft);
    assert!(!still[0].claims_submission());
}

#[test]
fn a_failed_save_after_a_send_keeps_possible_delivery() {
    for store_error in [StoreError::Unavailable, StoreError::Uncertain] {
        let gateway = Scripted::new(
            vec![SendAnswer::Receipt(Delivery::Queued)],
            vec![LookupAnswer::NotFound],
        );
        let seen = gateway.clone();
        let mut outbox = PhoneOutbox::new(
            Memory {
                rows: BTreeMap::new(),
                events: BTreeMap::new(),
                uncertain_depart: false,
                fail_answer: Some(store_error),
            },
            gateway,
            SeqIds::default(),
            clock(),
        );
        person_hold(&mut outbox, text_intent("command-1", "hello"));
        assert_eq!(
            outbox.send(&command("command-1")).unwrap_err(),
            OutboxError::Unsaved
        );
        let still = outbox.list().unwrap();
        assert_eq!(still.len(), 1);
        assert_eq!(
            still[0].outcome(),
            PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
        );
        assert!(!still[0].claims_submission());
        assert_eq!(still[0].intent().text(), "hello");
        let again = outbox.send(&command("command-1")).unwrap();
        assert_eq!(
            again.outcome(),
            PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
        );
        let body = seen.body();
        assert_eq!(body.sent.len(), 1);
        assert_eq!(body.order, ["send", "lookup"]);
    }
}

#[test]
fn a_direct_answer_beats_a_lookup() {
    let intent = text_intent("command-1", "hello");
    let planted = intent_record(&intent, PhoneOutcome::Done);
    let mut outbox = PhoneOutbox::new(
        PlantOn {
            rows: BTreeMap::new(),
            events: BTreeMap::new(),
            when: PhoneOutcome::Accepted,
            planted: Some(Planted {
                record: planted,
                events: vec![audit(Cause::Lookup, PhoneOutcome::Done)],
            }),
        },
        Scripted::new(vec![SendAnswer::Receipt(Delivery::Queued)], Vec::new()),
        SeqIds::default(),
        clock(),
    );
    person_hold(&mut outbox, intent);
    let view = outbox.send(&command("command-1")).unwrap();
    assert_eq!(view.outcome(), PhoneOutcome::Accepted);
    assert!(view.claims_submission());
    assert_eq!(view.intent().text(), "hello");
    let again = outbox.send(&command("command-1")).unwrap();
    assert_eq!(again.outcome(), PhoneOutcome::Accepted);
}

#[test]
fn a_lookup_does_not_replace_a_direct_answer() {
    let intent = text_intent("command-1", "hello");
    let departed = intent_record(&intent, PhoneOutcome::Unknown(UnknownCause::NotAdmitted));
    let mut rows = BTreeMap::new();
    rows.insert("command-1".to_owned(), departed);
    let mut events = BTreeMap::new();
    events.insert(
        "command-1".to_owned(),
        vec![
            audit(Cause::Held, PhoneOutcome::Draft),
            audit(
                Cause::Departed,
                PhoneOutcome::Unknown(UnknownCause::NotAdmitted),
            ),
        ],
    );
    let mut outbox = PhoneOutbox::new(
        PlantOn {
            rows,
            events,
            when: PhoneOutcome::Done,
            planted: Some(Planted {
                record: intent_record(&intent, PhoneOutcome::Accepted),
                events: vec![audit(Cause::SendAnswered, PhoneOutcome::Accepted)],
            }),
        },
        Scripted::new(Vec::new(), vec![settled(ReceiptEffect::Dispatched)]),
        SeqIds::default(),
        clock(),
    );
    let recovered = outbox.recover().unwrap();
    assert_eq!(recovered[0].outcome(), PhoneOutcome::Accepted);
    assert!(recovered[0].claims_submission());
    assert_eq!(recovered[0].intent().text(), "hello");
    let again = outbox.recover().unwrap();
    assert!(again.is_empty());
}

fn intent_record(intent: &PhoneIntent, outcome: PhoneOutcome) -> PhoneRecord {
    PhoneRecord::held(intent.clone())
        .depart()
        .unwrap()
        .with_sends(1)
        .with_outcome(outcome)
}

fn audit(cause: Cause, outcome: PhoneOutcome) -> IntentEvent {
    IntentEvent {
        cause,
        outcome,
        initiator: Initiator::Person,
        observed_at_ms: 1,
    }
}

/// The first replace that would store `when` instead leaves `planted` committed
/// and reports a conflict, as another connection's save would.
struct PlantOn {
    rows: BTreeMap<String, PhoneRecord>,
    events: BTreeMap<String, Vec<IntentEvent>>,
    when: PhoneOutcome,
    planted: Option<Planted>,
}

struct Planted {
    record: PhoneRecord,
    events: Vec<IntentEvent>,
}

impl IntentStore for PlantOn {
    fn load(&mut self, id: &PhoneCommandId) -> Result<Option<PhoneRecord>, StoreError> {
        Ok(self.rows.get(id.as_str()).cloned())
    }
    fn insert(&mut self, record: &PhoneRecord, event: IntentEvent) -> Result<(), StoreError> {
        let id = record.intent().command().as_str().to_owned();
        self.events.entry(id.clone()).or_default().push(event);
        self.rows.insert(id, record.clone());
        Ok(())
    }
    fn replace(
        &mut self,
        previous: &PhoneRecord,
        next: &PhoneRecord,
        event: IntentEvent,
    ) -> Result<(), StoreError> {
        if next.outcome() == self.when {
            if let Some(planted) = self.planted.take() {
                let id = planted.record.intent().command().as_str().to_owned();
                self.events.insert(id.clone(), planted.events);
                self.rows.insert(id, planted.record);
                return Err(StoreError::Conflict);
            }
        }
        let id = next.intent().command().as_str().to_owned();
        self.events.entry(id.clone()).or_default().push(event);
        self.rows.insert(id, next.clone());
        let _ = previous;
        Ok(())
    }
    fn replace_and_insert(
        &mut self,
        previous: &PhoneRecord,
        next: &PhoneRecord,
        draft: &PhoneRecord,
        events: &[IntentEvent],
        draft_event: IntentEvent,
    ) -> Result<(), StoreError> {
        self.replace(previous, next, events[0])?;
        for event in events.iter().skip(1) {
            self.events
                .entry(next.intent().command().as_str().to_owned())
                .or_default()
                .push(*event);
        }
        self.insert(draft, draft_event)
    }
    fn list(&mut self) -> Result<Vec<PhoneRecord>, StoreError> {
        Ok(self.rows.values().cloned().collect())
    }
    fn events(&mut self, id: &PhoneCommandId) -> Result<Vec<IntentEvent>, StoreError> {
        Ok(self.events.get(id.as_str()).cloned().unwrap_or_default())
    }
    fn begin_exclusive(&mut self) -> Result<(), StoreError> {
        Ok(())
    }
    fn end_exclusive(&mut self) -> Result<(), StoreError> {
        Ok(())
    }
    fn abort_exclusive(&mut self) {}
}

fn accepted_send(delivery: Delivery, outcome: PhoneOutcome) {
    let dir = directory();
    let gateway = Scripted::new(vec![SendAnswer::Receipt(delivery)], Vec::new());
    let seen = gateway.clone();
    let mut outbox = opened(&path_in(&dir), gateway);
    person_hold(&mut outbox, text_intent("command-1", "hello"));
    let view = outbox.send(&command("command-1")).unwrap();
    assert_eq!(view.outcome(), outcome);
    assert!(view.claims_submission());
    assert_eq!(view.intent().text(), "hello");
    assert_eq!(seen.body().sent.len(), 1);
    let again = outbox.send(&command("command-1")).unwrap();
    assert_eq!(again.outcome(), outcome);
    assert_eq!(seen.body().sent.len(), 1);
}

#[test]
fn a_queued_send_is_accepted() {
    accepted_send(Delivery::Queued, PhoneOutcome::Accepted);
}

#[test]
fn an_injected_send_is_running() {
    accepted_send(Delivery::Injected, PhoneOutcome::Running);
}

#[test]
fn a_settled_send_is_done() {
    accepted_send(Delivery::Settled, PhoneOutcome::Done);
}

#[test]
fn a_refusal_is_failed_and_not_sent_again() {
    let dir = directory();
    let gateway = Scripted::new(
        vec![failure(ConversationErrorCode::TemporarilyUnavailable)],
        Vec::new(),
    );
    let seen = gateway.clone();
    let mut outbox = opened(&path_in(&dir), gateway);
    person_hold(&mut outbox, text_intent("command-1", "hello"));
    let view = outbox.send(&command("command-1")).unwrap();
    assert_eq!(
        view.outcome(),
        PhoneOutcome::Failed(PhoneFailure::Conversation(
            ConversationErrorCode::TemporarilyUnavailable
        ))
    );
    assert!(!view.claims_submission());
    assert_eq!(view.intent().text(), "hello");
    let retried = outbox.retry(&command("command-1")).unwrap();
    assert_eq!(retried.outcome(), view.outcome());
    outbox.send(&command("command-1")).unwrap();
    assert_eq!(seen.body().sent.len(), 1);
    assert!(seen.body().looked.is_empty());
}

#[test]
fn turn_running_returns_the_text_to_a_new_draft() {
    let dir = directory();
    let gateway = Scripted::new(
        vec![failure(ConversationErrorCode::TurnRunning)],
        Vec::new(),
    );
    let seen = gateway.clone();
    let mut outbox = opened(&path_in(&dir), gateway);
    person_hold(&mut outbox, text_intent("command-1", "keep this"));
    let view = outbox.send(&command("command-1")).unwrap();
    assert_eq!(
        view.outcome(),
        PhoneOutcome::Failed(PhoneFailure::Conversation(
            ConversationErrorCode::TurnRunning
        ))
    );
    let successor = view.successor().unwrap().as_str().to_owned();
    assert_ne!(successor, "command-1");
    let views = outbox.list().unwrap();
    let draft = views
        .iter()
        .find(|view| view.intent().command().as_str() == successor)
        .unwrap();
    assert_eq!(draft.outcome(), PhoneOutcome::Draft);
    assert_eq!(draft.intent().text(), "keep this");
    assert!(!draft.claims_submission());
    outbox.retry(&command("command-1")).unwrap();
    assert_eq!(seen.body().sent.len(), 1);
    drop(outbox);
    let mut store = SqliteIntents::open(&path_in(&dir)).unwrap();
    let events = store.events("command-1").unwrap();
    let causes: Vec<_> = events.iter().map(|event| event.cause).collect();
    assert_eq!(
        causes,
        [Cause::Held, Cause::Departed, Cause::ReturnedToDraft]
    );
    assert!(events
        .iter()
        .all(|event| event.initiator == Initiator::Person));
    let draft_events = store.events(&successor).unwrap();
    assert_eq!(draft_events[0].cause, Cause::Held);
    assert_eq!(draft_events[0].initiator, Initiator::Person);
}

#[test]
fn a_lookup_turn_running_stays_a_lookup_result() {
    let dir = directory();
    let path = path_in(&dir);
    depart(&path, text_intent("command-1", "keep this"));
    let gateway = Scripted::new(
        Vec::new(),
        vec![failure_lookup(ConversationErrorCode::TurnRunning)],
    );
    let mut outbox = opened(&path, gateway);
    let recovered = outbox.recover().unwrap();
    assert_eq!(
        recovered[0].outcome(),
        PhoneOutcome::Failed(PhoneFailure::Conversation(
            ConversationErrorCode::TurnRunning
        ))
    );
    let successor = recovered[0].successor().unwrap().as_str().to_owned();
    drop(outbox);
    let mut store = SqliteIntents::open(&path).unwrap();
    let events = store.events("command-1").unwrap();
    let causes: Vec<_> = events.iter().map(|event| event.cause).collect();
    assert_eq!(
        causes,
        [
            Cause::Held,
            Cause::Departed,
            Cause::Lookup,
            Cause::ReturnedToDraft,
        ]
    );
    assert_eq!(events[2].initiator, Initiator::Recovery);
    assert_eq!(events[3].initiator, Initiator::Recovery);
    assert_eq!(events[2].outcome, events[3].outcome);
    let draft_events = store.events(&successor).unwrap();
    assert_eq!(draft_events.len(), 1);
    assert_eq!(draft_events[0].cause, Cause::Held);
    assert_eq!(draft_events[0].initiator, Initiator::Recovery);
    assert_eq!(draft_events[0].outcome, PhoneOutcome::Draft);
    let mut check = opened(&path, Scripted::new(Vec::new(), Vec::new()));
    assert_eq!(check.list().unwrap().len(), 2);
}

#[test]
fn a_lowered_grant_stays_failed() {
    refused_and_kept(
        SendAnswer::Refused(PhoneFailure::Forbidden),
        PhoneOutcome::Failed(PhoneFailure::Forbidden),
    );
}

#[test]
fn an_unenforceable_drive_policy_stays_failed() {
    refused_and_kept(
        failure(ConversationErrorCode::DrivePolicyUnenforceable),
        PhoneOutcome::Failed(PhoneFailure::Conversation(
            ConversationErrorCode::DrivePolicyUnenforceable,
        )),
    );
}

fn refused_and_kept(answer: SendAnswer, outcome: PhoneOutcome) {
    let dir = directory();
    let gateway = Scripted::new(vec![answer], Vec::new());
    let seen = gateway.clone();
    let mut outbox = opened(&path_in(&dir), gateway);
    person_hold(&mut outbox, text_intent("command-1", "hello"));
    let view = outbox.send(&command("command-1")).unwrap();
    assert_eq!(view.outcome(), outcome);
    assert_eq!(view.intent().text(), "hello");
    assert!(!view.claims_submission());
    outbox.retry(&command("command-1")).unwrap();
    assert_eq!(seen.body().sent.len(), 1);
}

#[test]
fn a_deleted_conversation_keeps_the_typed_text() {
    refused_and_kept(
        failure(ConversationErrorCode::ConversationDeleted),
        PhoneOutcome::Failed(PhoneFailure::Conversation(
            ConversationErrorCode::ConversationDeleted,
        )),
    );
}

#[test]
fn submission_unresolved_is_not_sent_again() {
    let dir = directory();
    let gateway = Scripted::new(
        vec![failure(ConversationErrorCode::SubmissionUnresolved)],
        Vec::new(),
    );
    let seen = gateway.clone();
    let mut outbox = opened(&path_in(&dir), gateway);
    person_hold(&mut outbox, text_intent("command-1", "hello"));
    let view = outbox.send(&command("command-1")).unwrap();
    assert_eq!(
        view.outcome(),
        PhoneOutcome::Unknown(UnknownCause::Unresolved)
    );
    assert!(!view.claims_submission());
    outbox.retry(&command("command-1")).unwrap();
    outbox.send(&command("command-1")).unwrap();
    assert_eq!(seen.body().sent.len(), 1);
    assert!(seen.body().looked.is_empty());
}

#[test]
fn a_lost_answer_with_an_accepted_receipt_is_not_sent_again() {
    let dir = directory();
    let gateway = Scripted::new(
        vec![SendAnswer::Lost],
        vec![LookupAnswer::Found {
            stage: ReceiptStage::Accepted,
            effect: None,
        }],
    );
    let seen = gateway.clone();
    let mut outbox = opened(&path_in(&dir), gateway);
    person_hold(&mut outbox, text_intent("command-1", "hello"));
    let view = outbox.send(&command("command-1")).unwrap();
    assert_eq!(view.outcome(), PhoneOutcome::Accepted);
    assert!(view.claims_submission());
    let body = seen.body();
    assert_eq!(body.sent.len(), 1);
    assert_eq!(body.looked.len(), 1);
    assert_eq!(body.sent[0], body.looked[0]);
}

#[test]
fn an_attempted_receipt_is_unknown_and_not_sent_again() {
    let dir = directory();
    let gateway = Scripted::new(
        vec![SendAnswer::Lost],
        vec![LookupAnswer::Found {
            stage: ReceiptStage::Attempted,
            effect: None,
        }],
    );
    let seen = gateway.clone();
    let mut outbox = opened(&path_in(&dir), gateway);
    person_hold(&mut outbox, text_intent("command-1", "hello"));
    let view = outbox.send(&command("command-1")).unwrap();
    assert_eq!(
        view.outcome(),
        PhoneOutcome::Unknown(UnknownCause::Attempted)
    );
    outbox.retry(&command("command-1")).unwrap();
    assert_eq!(seen.body().sent.len(), 1);
    assert_eq!(seen.body().looked.len(), 1);
}

#[test]
fn a_dispatched_receipt_is_done() {
    let dir = directory();
    let gateway = Scripted::new(
        vec![SendAnswer::Lost],
        vec![settled(ReceiptEffect::Dispatched)],
    );
    let mut outbox = opened(&path_in(&dir), gateway);
    person_hold(&mut outbox, text_intent("command-1", "hello"));
    let view = outbox.send(&command("command-1")).unwrap();
    assert_eq!(view.outcome(), PhoneOutcome::Done);
    assert!(view.claims_submission());
}

#[test]
fn a_settled_refusal_is_failed() {
    let cases = [
        (ReceiptEffect::NoEffect, ConversationErrorCode::Revoked),
        (
            ReceiptEffect::RefusedAfterSave,
            ConversationErrorCode::RevokedAfterSave,
        ),
        (
            ReceiptEffect::UnavailableAfterSave,
            ConversationErrorCode::UnavailableAfterSave,
        ),
    ];
    for (effect, code) in cases {
        let dir = directory();
        let gateway = Scripted::new(vec![SendAnswer::Lost], vec![settled(effect)]);
        let seen = gateway.clone();
        let mut outbox = opened(&path_in(&dir), gateway);
        person_hold(&mut outbox, text_intent("command-1", "hello"));
        let view = outbox.send(&command("command-1")).unwrap();
        assert_eq!(
            view.outcome(),
            PhoneOutcome::Failed(PhoneFailure::Conversation(code))
        );
        assert_eq!(view.intent().text(), "hello");
        assert_eq!(seen.body().sent.len(), 1);
    }
}

#[test]
fn a_lost_answer_retransmits_the_same_bytes_until_the_bound() {
    let dir = directory();
    let path = path_in(&dir);
    let gateway = Scripted::new(
        vec![SendAnswer::Lost, SendAnswer::Receipt(Delivery::Queued)],
        vec![LookupAnswer::NotFound],
    );
    let seen = gateway.clone();
    let mut outbox = opened(&path, gateway);
    let saved = text_intent("command-1", "hello");
    let canonical = saved.canonical().to_vec();
    person_hold(&mut outbox, saved);
    let view = outbox.send(&command("command-1")).unwrap();
    assert_eq!(view.outcome(), PhoneOutcome::Accepted);
    let body = seen.body();
    assert_eq!(body.sent.len(), 2);
    assert_eq!(body.sent[0], canonical);
    assert_eq!(body.sent[1], canonical);
    assert_eq!(body.order, ["send", "lookup", "send"]);
    drop(body);
    drop(outbox);
    let mut store = SqliteIntents::open(&path).unwrap();
    let foreground: Vec<_> = store
        .events("command-1")
        .unwrap()
        .into_iter()
        .filter(|event| event.cause == Cause::ForegroundSend)
        .collect();
    assert_eq!(foreground.len(), 1);
    assert_eq!(foreground[0].outcome, PhoneOutcome::Accepted);
}

#[test]
fn foreground_send_stops_at_the_bound() {
    let sends = vec![SendAnswer::Lost; usize::from(FOREGROUND_SENDS)];
    let lookups = vec![LookupAnswer::NotFound; usize::from(FOREGROUND_SENDS)];
    let dir = directory();
    let path = path_in(&dir);
    let gateway = Scripted::new(sends, lookups);
    let seen = gateway.clone();
    let mut outbox = opened(&path, gateway);
    let saved = text_intent("command-1", "hello");
    let canonical = saved.canonical().to_vec();
    person_hold(&mut outbox, saved);
    let view = outbox.send(&command("command-1")).unwrap();
    assert_eq!(
        view.outcome(),
        PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
    );
    assert!(!view.claims_submission());
    let body = seen.body();
    assert_eq!(body.sent.len(), usize::from(FOREGROUND_SENDS));
    assert!(body.sent.iter().all(|bytes| bytes == &canonical));
    assert_eq!(body.looked.len(), usize::from(FOREGROUND_SENDS));
    drop(body);
    drop(outbox);
    let mut store = SqliteIntents::open(&path).unwrap();
    let events = store.events("command-1").unwrap();
    assert_eq!(events.last().unwrap().cause, Cause::Bound);
    assert_eq!(
        events.last().unwrap().outcome,
        PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
    );
    assert!(events.iter().all(|event| event.cause != Cause::Lookup));
}

#[test]
fn a_lookup_that_cannot_be_asked_stays_unknown() {
    let dir = directory();
    let gateway = Scripted::new(vec![SendAnswer::Lost], vec![LookupAnswer::Lost]);
    let seen = gateway.clone();
    let mut outbox = opened(&path_in(&dir), gateway);
    person_hold(&mut outbox, text_intent("command-1", "hello"));
    let view = outbox.send(&command("command-1")).unwrap();
    assert_eq!(
        view.outcome(),
        PhoneOutcome::Unknown(UnknownCause::LookupFailed)
    );
    assert!(!view.claims_submission());
    assert_eq!(seen.body().sent.len(), 1);
    assert_eq!(seen.body().looked.len(), 1);
}

#[test]
fn an_unreadable_send_answer_is_not_sent_again() {
    let dir = directory();
    let gateway = Scripted::new(vec![SendAnswer::Broken], vec![LookupAnswer::NotFound]);
    let seen = gateway.clone();
    let mut outbox = opened(&path_in(&dir), gateway);
    person_hold(&mut outbox, text_intent("command-1", "hello"));
    let view = outbox.send(&command("command-1")).unwrap();
    assert_eq!(
        view.outcome(),
        PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
    );
    let body = seen.body();
    assert_eq!(body.sent.len(), 1);
    assert_eq!(body.order, ["send", "lookup"]);
}

#[test]
fn a_conflicting_lookup_is_failed() {
    let dir = directory();
    let gateway = Scripted::new(vec![SendAnswer::Lost], vec![LookupAnswer::Conflict]);
    let seen = gateway.clone();
    let mut outbox = opened(&path_in(&dir), gateway);
    person_hold(&mut outbox, text_intent("command-1", "hello"));
    let view = outbox.send(&command("command-1")).unwrap();
    assert_eq!(
        view.outcome(),
        PhoneOutcome::Failed(PhoneFailure::Conversation(
            ConversationErrorCode::SubmissionConflict
        ))
    );
    outbox.retry(&command("command-1")).unwrap();
    assert_eq!(seen.body().sent.len(), 1);
    assert_eq!(seen.body().looked.len(), 1);
}

#[test]
fn recovery_does_not_send_when_the_receipt_is_absent() {
    let dir = directory();
    let path = path_in(&dir);
    depart(&path, text_intent("command-1", "hello"));
    let gateway = Scripted::new(
        Vec::new(),
        vec![LookupAnswer::NotFound, LookupAnswer::NotFound],
    );
    let seen = gateway.clone();
    let mut outbox = opened(&path, gateway);
    let recovered = outbox.recover().unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].outcome(),
        PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
    );
    assert!(!recovered[0].claims_submission());
    assert_eq!(recovered[0].intent().text(), "hello");
    outbox.send(&command("command-1")).unwrap();
    let body = seen.body();
    assert!(body.sent.is_empty());
    assert_eq!(body.order, ["lookup", "lookup"]);
}

#[test]
fn recovery_adopts_a_dispatched_receipt_without_sending() {
    let dir = directory();
    let path = path_in(&dir);
    depart(&path, text_intent("command-1", "hello"));
    let gateway = Scripted::new(Vec::new(), vec![settled(ReceiptEffect::Dispatched)]);
    let seen = gateway.clone();
    let mut outbox = opened(&path, gateway);
    let recovered = outbox.recover().unwrap();
    assert_eq!(recovered[0].outcome(), PhoneOutcome::Done);
    assert!(recovered[0].claims_submission());
    assert!(seen.body().sent.is_empty());
    assert_eq!(seen.body().order, ["lookup"]);
    drop(outbox);
    let mut store = SqliteIntents::open(&path).unwrap();
    let events = store.events("command-1").unwrap();
    assert_eq!(events[0].initiator, Initiator::Person);
    assert_eq!(events.last().unwrap().initiator, Initiator::Recovery);
}

#[test]
fn a_lookup_failure_is_retried_as_an_explicit_send() {
    let dir = directory();
    let path = path_in(&dir);
    {
        let gateway = Scripted::new(vec![SendAnswer::Lost], vec![LookupAnswer::Lost]);
        let mut outbox = opened(&path, gateway);
        person_hold(&mut outbox, text_intent("command-1", "hello"));
        let view = outbox.send(&command("command-1")).unwrap();
        assert_eq!(
            view.outcome(),
            PhoneOutcome::Unknown(UnknownCause::LookupFailed)
        );
    }
    let gateway = Scripted::new(
        vec![SendAnswer::Receipt(Delivery::Queued)],
        vec![LookupAnswer::NotFound],
    );
    let mut outbox = opened(&path, gateway);
    let view = outbox.retry(&command("command-1")).unwrap();
    assert_eq!(view.outcome(), PhoneOutcome::Accepted);
    drop(outbox);
    let mut check = opened(&path, Scripted::new(Vec::new(), Vec::new()));
    let rows = check.list().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].outcome(), PhoneOutcome::Accepted);
    drop(check);
    let mut store = SqliteIntents::open(&path).unwrap();
    let events = store.events("command-1").unwrap();
    let causes: Vec<_> = events.iter().map(|event| event.cause).collect();
    assert_eq!(
        causes,
        [
            Cause::Held,
            Cause::Departed,
            Cause::SendLost,
            Cause::Lookup,
            Cause::Lookup,
            Cause::ExplicitRetry,
            Cause::ExplicitRetry,
        ]
    );
    assert_eq!(
        events[3].outcome,
        PhoneOutcome::Unknown(UnknownCause::LookupFailed)
    );
    assert_eq!(
        events[4].outcome,
        PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
    );
    assert_eq!(
        store.load(&command("command-1")).unwrap().unwrap().sends(),
        1
    );
}

#[test]
fn an_explicit_retry_looks_up_before_it_sends() {
    let dir = directory();
    let path = path_in(&dir);
    let saved = text_intent("command-1", "hello");
    let canonical = saved.canonical().to_vec();
    depart(&path, saved);
    let gateway = Scripted::new(
        vec![SendAnswer::Receipt(Delivery::Queued)],
        vec![LookupAnswer::NotFound],
    );
    let seen = gateway.clone();
    let mut outbox = opened(&path, gateway);
    let view = outbox.retry(&command("command-1")).unwrap();
    assert_eq!(view.outcome(), PhoneOutcome::Accepted);
    {
        let body = seen.body();
        assert_eq!(body.order, ["lookup", "send"]);
        assert_eq!(body.sent[0], canonical);
        assert_eq!(body.looked[0], canonical);
    }
    drop(outbox);
    let mut store = SqliteIntents::open(&path).unwrap();
    let events = store.events("command-1").unwrap();
    let causes: Vec<_> = events.iter().map(|event| event.cause).collect();
    assert_eq!(
        causes,
        [
            Cause::Held,
            Cause::Departed,
            Cause::Lookup,
            Cause::ExplicitRetry,
            Cause::ExplicitRetry,
        ]
    );
    assert_eq!(
        events[2].outcome,
        PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
    );
    assert_eq!(
        events[3].outcome,
        PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
    );
    assert_eq!(events[4].outcome, PhoneOutcome::Accepted);
}

#[test]
fn a_concurrent_retry_sends_the_departure_once() {
    let dir = directory();
    let path = path_in(&dir);
    depart(&path, text_intent("command-1", "hello"));
    let entered = Arc::new(Barrier::new(2));
    let sends = Arc::new(AtomicUsize::new(0));
    let spawn = |path: PathBuf, entered: Arc<Barrier>, sends: Arc<AtomicUsize>| {
        thread::spawn(move || {
            let mut outbox = open(
                &path,
                SharedRetry { entered, sends },
                SeqIds::default(),
                clock(),
            )
            .unwrap();
            outbox.retry(&command("command-1")).unwrap().outcome()
        })
    };
    let first = spawn(path.clone(), Arc::clone(&entered), Arc::clone(&sends));
    let second = spawn(path, entered, sends.clone());
    let left = first.join().unwrap();
    let right = second.join().unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    assert_eq!(left, PhoneOutcome::Accepted);
    assert_eq!(right, PhoneOutcome::Accepted);
}

struct SharedRetry {
    entered: Arc<Barrier>,
    sends: Arc<AtomicUsize>,
}
impl PhoneGateway for SharedRetry {
    fn send(&mut self, _: &PhoneIntent) -> SendAnswer {
        self.sends.fetch_add(1, Ordering::SeqCst);
        SendAnswer::Receipt(Delivery::Queued)
    }
    fn lookup(&mut self, _: &PhoneIntent) -> LookupAnswer {
        self.entered.wait();
        LookupAnswer::NotFound
    }
}

#[test]
fn a_concurrent_hold_of_the_same_bytes_keeps_one_draft() {
    let dir = directory();
    let path = path_in(&dir);
    drop(opened(&path, Scripted::new(Vec::new(), Vec::new())));
    let ready = Arc::new(Barrier::new(3));
    let go = Arc::new(Barrier::new(3));
    let spawn = |path: PathBuf, ready: Arc<Barrier>, go: Arc<Barrier>| {
        thread::spawn(move || {
            let mut outbox = open(
                &path,
                Scripted::new(Vec::new(), Vec::new()),
                SeqIds::default(),
                clock(),
            )
            .unwrap();
            ready.wait();
            go.wait();
            outbox
                .hold(text_intent("command-1", "hello"))
                .unwrap()
                .outcome()
        })
    };
    let left = spawn(path.clone(), Arc::clone(&ready), Arc::clone(&go));
    let right = spawn(path.clone(), Arc::clone(&ready), Arc::clone(&go));
    ready.wait();
    let connection = Connection::open(&path).unwrap();
    connection.busy_timeout(Duration::from_secs(5)).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    go.wait();
    thread::sleep(Duration::from_millis(200));
    connection.execute_batch("ROLLBACK").unwrap();
    assert_eq!(left.join().unwrap(), PhoneOutcome::Draft);
    assert_eq!(right.join().unwrap(), PhoneOutcome::Draft);
    let mut check = opened(&path, Scripted::new(Vec::new(), Vec::new()));
    let rows = check.list().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].outcome(), PhoneOutcome::Draft);
    assert_eq!(rows[0].intent().text(), "hello");
}

#[test]
fn a_lost_departure_does_not_send_again() {
    let dir = directory();
    let path = path_in(&dir);
    {
        let mut outbox = opened(&path, Scripted::new(Vec::new(), Vec::new()));
        person_hold(&mut outbox, text_intent("command-1", "hello"));
    }
    let sends = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new((Mutex::new(false), Condvar::new()));
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let (done_tx, done_rx) = mpsc::channel();
    let spawn = |path: PathBuf| {
        let sends = Arc::clone(&sends);
        let entered = Arc::clone(&entered);
        let release = Arc::clone(&release);
        let done_tx = done_tx.clone();
        thread::spawn(move || {
            let mut outbox = open(
                &path,
                PausingSend {
                    sends,
                    entered,
                    release,
                },
                SeqIds::default(),
                clock(),
            )
            .unwrap();
            let outcome = outbox.send(&command("command-1")).unwrap().outcome();
            done_tx.send(outcome).unwrap();
        })
    };
    let first = spawn(path.clone());
    let second = spawn(path.clone());
    {
        let (lock, cv) = &*entered;
        let mut flag = lock.lock().unwrap();
        while !*flag {
            flag = cv.wait(flag).unwrap();
        }
    }
    assert!(done_rx.recv_timeout(Duration::from_millis(400)).is_err());
    {
        let (lock, cv) = &*release;
        let mut flag = lock.lock().unwrap();
        *flag = true;
        cv.notify_all();
    }
    let left = done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let right = done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    first.join().unwrap();
    second.join().unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    assert_eq!(left, PhoneOutcome::Accepted);
    assert_eq!(right, PhoneOutcome::Accepted);
    let mut check = opened(&path, Scripted::new(Vec::new(), Vec::new()));
    let rows = check.list().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].intent().command().as_str(), "command-1");
    assert_eq!(rows[0].outcome(), PhoneOutcome::Accepted);
}

#[test]
fn a_spent_budget_keeps_lookup_failed() {
    let dir = directory();
    let path = path_in(&dir);
    {
        let mut outbox = opened(&path, Scripted::new(Vec::new(), Vec::new()));
        person_hold(&mut outbox, text_intent("command-1", "hello"));
    }
    let sends = Arc::new(AtomicUsize::new(0));
    let lookups = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new((Mutex::new(false), Condvar::new()));
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let ready = Arc::new(Barrier::new(3));
    let go = Arc::new(Barrier::new(3));
    let (done_tx, done_rx) = mpsc::channel();
    let spawn = |path: PathBuf| {
        let sends = Arc::clone(&sends);
        let lookups = Arc::clone(&lookups);
        let entered = Arc::clone(&entered);
        let release = Arc::clone(&release);
        let ready = Arc::clone(&ready);
        let go = Arc::clone(&go);
        let done_tx = done_tx.clone();
        thread::spawn(move || {
            let mut outbox = open(
                &path,
                BudgetPause {
                    sends,
                    lookups,
                    entered,
                    release,
                },
                SeqIds::default(),
                clock(),
            )
            .unwrap();
            ready.wait();
            go.wait();
            let outcome = outbox.send(&command("command-1")).unwrap().outcome();
            done_tx.send(outcome).unwrap();
        })
    };
    let first = spawn(path.clone());
    let second = spawn(path.clone());
    ready.wait();
    let connection = Connection::open(&path).unwrap();
    connection.busy_timeout(Duration::from_secs(5)).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    go.wait();
    thread::sleep(Duration::from_millis(200));
    connection.execute_batch("ROLLBACK").unwrap();
    {
        let (lock, cv) = &*entered;
        let mut flag = lock.lock().unwrap();
        while !*flag {
            flag = cv.wait(flag).unwrap();
        }
    }
    assert!(done_rx.recv_timeout(Duration::from_millis(400)).is_err());
    {
        let (lock, cv) = &*release;
        let mut flag = lock.lock().unwrap();
        *flag = true;
        cv.notify_all();
    }
    let left = done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let right = done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    first.join().unwrap();
    second.join().unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), usize::from(FOREGROUND_SENDS));
    assert_eq!(
        lookups.load(Ordering::SeqCst),
        usize::from(FOREGROUND_SENDS)
    );
    assert_eq!(left, PhoneOutcome::Unknown(UnknownCause::LookupFailed));
    assert_eq!(right, PhoneOutcome::Unknown(UnknownCause::LookupFailed));
    let mut check = opened(&path, Scripted::new(Vec::new(), Vec::new()));
    let rows = check.list().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].outcome(),
        PhoneOutcome::Unknown(UnknownCause::LookupFailed)
    );
    drop(check);
    let mut store = SqliteIntents::open(&path).unwrap();
    let record = store.load(&command("command-1")).unwrap().unwrap();
    assert_eq!(record.sends(), FOREGROUND_SENDS);
    let events = store.events("command-1").unwrap();
    assert_eq!(events.last().unwrap().cause, Cause::Lookup);
    assert!(events.iter().all(|event| event.cause != Cause::Bound));
}

struct BudgetPause {
    sends: Arc<AtomicUsize>,
    lookups: Arc<AtomicUsize>,
    entered: Arc<(Mutex<bool>, Condvar)>,
    release: Arc<(Mutex<bool>, Condvar)>,
}
impl PhoneGateway for BudgetPause {
    fn send(&mut self, _: &PhoneIntent) -> SendAnswer {
        let nth = self.sends.fetch_add(1, Ordering::SeqCst);
        if nth == 0 {
            let (lock, cv) = &*self.entered;
            let mut flag = lock.lock().unwrap();
            *flag = true;
            cv.notify_all();
            drop(flag);
            let (lock, cv) = &*self.release;
            let mut flag = lock.lock().unwrap();
            while !*flag {
                flag = cv.wait(flag).unwrap();
            }
        }
        SendAnswer::Lost
    }
    fn lookup(&mut self, _: &PhoneIntent) -> LookupAnswer {
        let nth = self.lookups.fetch_add(1, Ordering::SeqCst);
        if nth + 1 < usize::from(FOREGROUND_SENDS) {
            LookupAnswer::NotFound
        } else {
            LookupAnswer::Lost
        }
    }
}

struct PausingSend {
    sends: Arc<AtomicUsize>,
    entered: Arc<(Mutex<bool>, Condvar)>,
    release: Arc<(Mutex<bool>, Condvar)>,
}
impl PhoneGateway for PausingSend {
    fn send(&mut self, _: &PhoneIntent) -> SendAnswer {
        self.sends.fetch_add(1, Ordering::SeqCst);
        {
            let (lock, cv) = &*self.entered;
            let mut flag = lock.lock().unwrap();
            *flag = true;
            cv.notify_all();
        }
        let (lock, cv) = &*self.release;
        let mut flag = lock.lock().unwrap();
        while !*flag {
            flag = cv.wait(flag).unwrap();
        }
        SendAnswer::Receipt(Delivery::Queued)
    }
    fn lookup(&mut self, _: &PhoneIntent) -> LookupAnswer {
        LookupAnswer::NotFound
    }
}

#[test]
fn recovery_adopts_a_settled_receipt_after_attempted() {
    let dir = directory();
    let path = path_in(&dir);
    {
        let gateway = Scripted::new(
            vec![SendAnswer::Lost],
            vec![LookupAnswer::Found {
                stage: ReceiptStage::Attempted,
                effect: None,
            }],
        );
        let mut outbox = opened(&path, gateway);
        person_hold(&mut outbox, text_intent("command-1", "hello"));
        let view = outbox.send(&command("command-1")).unwrap();
        assert_eq!(
            view.outcome(),
            PhoneOutcome::Unknown(UnknownCause::Attempted)
        );
    }
    let gateway = Scripted::new(Vec::new(), vec![settled(ReceiptEffect::Dispatched)]);
    let seen = gateway.clone();
    let mut outbox = opened(&path, gateway);
    let recovered = outbox.recover().unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].outcome(), PhoneOutcome::Done);
    assert!(recovered[0].claims_submission());
    assert_eq!(recovered[0].intent().text(), "hello");
    assert!(seen.body().sent.is_empty());
    assert_eq!(seen.body().order, ["lookup"]);
}

#[test]
fn an_explicit_retry_does_not_send_an_uncertain_or_finished_command() {
    let dir = directory();
    let gateway = Scripted::new(
        vec![
            failure(ConversationErrorCode::SubmissionUnresolved),
            SendAnswer::Lost,
            failure(ConversationErrorCode::TemporarilyUnavailable),
            SendAnswer::Receipt(Delivery::Settled),
        ],
        vec![LookupAnswer::Found {
            stage: ReceiptStage::Attempted,
            effect: None,
        }],
    );
    let seen = gateway.clone();
    let mut outbox = opened(&path_in(&dir), gateway);
    for (id, _label) in [
        ("unresolved", "unresolved"),
        ("attempted", "attempted"),
        ("failed", "failed"),
        ("done", "done"),
    ] {
        person_hold(&mut outbox, text_intent(id, "hello"));
        outbox.send(&command(id)).unwrap();
    }
    for id in ["unresolved", "attempted", "failed", "done"] {
        outbox.retry(&command(id)).unwrap();
    }
    let body = seen.body();
    assert_eq!(body.sent.len(), 4);
    assert_eq!(body.looked.len(), 1);
}

#[test]
fn corrupt_canonical_bytes_are_left_in_place() {
    let dir = directory();
    let path = path_in(&dir);
    {
        let gateway = Scripted::new(Vec::new(), Vec::new());
        let mut outbox = opened(&path, gateway);
        person_hold(&mut outbox, text_intent("command-1", "hello"));
    }
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE phone_intent SET canonical = ?1 WHERE request_id = 'command-1'",
            params![b"not-a-send".as_slice()],
        )
        .unwrap();
    drop(connection);
    let gateway = Scripted::new(Vec::new(), Vec::new());
    let seen = gateway.clone();
    let mut outbox = opened(&path, gateway);
    assert_eq!(outbox.list().unwrap_err(), OutboxError::Corrupt);
    assert!(seen.body().order.is_empty());
    let connection = Connection::open(&path).unwrap();
    let canonical: Vec<u8> = connection
        .query_row(
            "SELECT canonical FROM phone_intent WHERE request_id = 'command-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(canonical, b"not-a-send");
}

#[test]
fn a_done_row_whose_history_stops_at_departure_is_corrupt() {
    let dir = directory();
    let path = path_in(&dir);
    depart(&path, text_intent("command-1", "hello"));
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE phone_intent SET outcome = 'done', failure = NULL, unknown = NULL
             WHERE request_id = 'command-1'",
            [],
        )
        .unwrap();
    drop(connection);
    let gateway = Scripted::new(Vec::new(), Vec::new());
    let seen = gateway.clone();
    let mut outbox = opened(&path, gateway);
    assert_eq!(
        outbox.send(&command("command-1")).unwrap_err(),
        OutboxError::Corrupt
    );
    assert_eq!(outbox.list().unwrap_err(), OutboxError::Corrupt);
    assert!(seen.body().order.is_empty());
    let connection = Connection::open(&path).unwrap();
    let outcome: String = connection
        .query_row(
            "SELECT outcome FROM phone_intent WHERE request_id = 'command-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(outcome, "done");

    let dir = directory();
    let path = path_in(&dir);
    depart(&path, text_intent("command-2", "hello"));
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE phone_intent_event SET initiator = 'recovery' WHERE cause = 'departed'",
            [],
        )
        .unwrap();
    drop(connection);
    let mut outbox = opened(&path, Scripted::new(Vec::new(), Vec::new()));
    assert_eq!(
        outbox.send(&command("command-2")).unwrap_err(),
        OutboxError::Corrupt
    );
    let connection = Connection::open(&path).unwrap();
    let initiator: String = connection
        .query_row(
            "SELECT initiator FROM phone_intent_event WHERE cause = 'departed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(initiator, "recovery");
}

#[test]
fn an_attempted_receipt_is_not_restored_as_not_admitted() {
    let dir = directory();
    let path = path_in(&dir);
    {
        let gateway = Scripted::new(
            vec![SendAnswer::Lost],
            vec![LookupAnswer::Found {
                stage: ReceiptStage::Attempted,
                effect: None,
            }],
        );
        let mut outbox = opened(&path, gateway);
        person_hold(&mut outbox, text_intent("command-1", "hello"));
        let view = outbox.send(&command("command-1")).unwrap();
        assert_eq!(
            view.outcome(),
            PhoneOutcome::Unknown(UnknownCause::Attempted)
        );
    }
    let connection = Connection::open(&path).unwrap();
    let ordinal: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(ordinal), -1) + 1 FROM phone_intent_event
             WHERE request_id = 'command-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute(
            "UPDATE phone_intent SET outcome = 'unknown', unknown = 'not_admitted', failure = NULL
             WHERE request_id = 'command-1'",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO phone_intent_event
             (request_id, ordinal, cause, outcome, failure, unknown, initiator, observed_at_ms)
             VALUES ('command-1', ?1, 'lookup', 'unknown', NULL, 'not_admitted', 'person', 1)",
            params![ordinal],
        )
        .unwrap();
    drop(connection);
    let mut outbox = opened(&path, Scripted::new(Vec::new(), Vec::new()));
    assert_eq!(outbox.list().unwrap_err(), OutboxError::Corrupt);
    drop(outbox);
    let connection = Connection::open(&path).unwrap();
    let unknown: String = connection
        .query_row(
            "SELECT unknown FROM phone_intent WHERE request_id = 'command-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(unknown, "not_admitted");
}

#[test]
fn a_recovery_draft_requires_its_turn_running_predecessor() {
    let prepare = || {
        let dir = directory();
        let path = path_in(&dir);
        depart(&path, text_intent("command-1", "keep this"));
        let gateway = Scripted::new(
            Vec::new(),
            vec![failure_lookup(ConversationErrorCode::TurnRunning)],
        );
        let mut outbox = opened(&path, gateway);
        let recovered = outbox.recover().unwrap();
        let successor = recovered[0].successor().unwrap().as_str().to_owned();
        assert_eq!(outbox.list().unwrap().len(), 2);
        drop(outbox);
        (dir, path, successor)
    };

    let (_dir, path, successor) = prepare();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "DELETE FROM phone_intent WHERE request_id = 'command-1'",
            [],
        )
        .unwrap();
    drop(connection);
    let mut outbox = opened(&path, Scripted::new(Vec::new(), Vec::new()));
    assert_eq!(outbox.list().unwrap_err(), OutboxError::Corrupt);
    drop(outbox);
    let connection = Connection::open(&path).unwrap();
    let outcome: String = connection
        .query_row(
            "SELECT outcome FROM phone_intent WHERE request_id = ?1",
            params![successor.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(outcome, "draft");

    let (_dir, path, successor) = prepare();
    let duplicate = text_intent("command-2", "keep this");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO phone_intent
             SELECT 'command-2', ?1, departed, outcome, failure, unknown, sends, successor
             FROM phone_intent WHERE request_id = 'command-1'",
            params![duplicate.canonical()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO phone_intent_event
             SELECT 'command-2', ordinal, cause, outcome, failure, unknown, initiator, observed_at_ms
             FROM phone_intent_event WHERE request_id = 'command-1'",
            [],
        )
        .unwrap();
    drop(connection);
    let mut outbox = opened(&path, Scripted::new(Vec::new(), Vec::new()));
    assert_eq!(outbox.list().unwrap_err(), OutboxError::Corrupt);
    drop(outbox);
    let connection = Connection::open(&path).unwrap();
    let pointed: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM phone_intent WHERE successor = ?1",
            params![successor.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pointed, 2);

    let (_dir, path, _successor) = prepare();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE phone_intent SET outcome = 'done', failure = NULL, unknown = NULL
             WHERE request_id = 'command-1'",
            [],
        )
        .unwrap();
    drop(connection);
    let mut outbox = opened(&path, Scripted::new(Vec::new(), Vec::new()));
    assert_eq!(outbox.list().unwrap_err(), OutboxError::Corrupt);
    drop(outbox);
    let connection = Connection::open(&path).unwrap();
    let outcome: String = connection
        .query_row(
            "SELECT outcome FROM phone_intent WHERE request_id = 'command-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(outcome, "done");

    let (_dir, path, successor) = prepare();
    depart(&path, text_intent("command-2", "keep this"));
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE phone_intent SET successor = ?1 WHERE request_id = 'command-2'",
            params![successor.as_str()],
        )
        .unwrap();
    drop(connection);
    let mut outbox = opened(&path, Scripted::new(Vec::new(), Vec::new()));
    assert_eq!(outbox.list().unwrap_err(), OutboxError::Corrupt);

    let (_dir, path, _successor) = prepare();
    let other = text_intent("command-1", "other");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE phone_intent SET canonical = ?1 WHERE request_id = 'command-1'",
            params![other.canonical()],
        )
        .unwrap();
    drop(connection);
    let mut outbox = opened(&path, Scripted::new(Vec::new(), Vec::new()));
    assert_eq!(outbox.list().unwrap_err(), OutboxError::Corrupt);
    drop(outbox);
    let connection = Connection::open(&path).unwrap();
    let canonical: Vec<u8> = connection
        .query_row(
            "SELECT canonical FROM phone_intent WHERE request_id = 'command-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(canonical, other.canonical());

    let (_dir, path, successor) = prepare();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE phone_intent_event SET initiator = 'person'
             WHERE request_id = 'command-1'
               AND cause IN ('lookup', 'returned_to_draft')",
            [],
        )
        .unwrap();
    drop(connection);
    let mut outbox = opened(&path, Scripted::new(Vec::new(), Vec::new()));
    assert_eq!(outbox.list().unwrap_err(), OutboxError::Corrupt);
    drop(outbox);
    let connection = Connection::open(&path).unwrap();
    let initiator: String = connection
        .query_row(
            "SELECT initiator FROM phone_intent_event
             WHERE request_id = 'command-1' AND cause = 'lookup'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(initiator, "person");
    let draft: String = connection
        .query_row(
            "SELECT outcome FROM phone_intent WHERE request_id = ?1",
            params![successor.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(draft, "draft");
}

#[test]
fn a_send_count_the_replay_does_not_reach_is_corrupt() {
    let dir = directory();
    let path = path_in(&dir);
    {
        let gateway = Scripted::new(vec![SendAnswer::Receipt(Delivery::Queued)], Vec::new());
        let mut outbox = opened(&path, gateway);
        person_hold(&mut outbox, text_intent("command-1", "hello"));
        outbox.send(&command("command-1")).unwrap();
    }
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE phone_intent SET sends = 0 WHERE request_id = 'command-1'",
            [],
        )
        .unwrap();
    drop(connection);
    let mut outbox = opened(&path, Scripted::new(Vec::new(), Vec::new()));
    assert_eq!(outbox.list().unwrap_err(), OutboxError::Corrupt);
    drop(outbox);
    let connection = Connection::open(&path).unwrap();
    let sends: i64 = connection
        .query_row(
            "SELECT sends FROM phone_intent WHERE request_id = 'command-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(sends, 0);
}

#[test]
fn a_recovery_of_lookup_failed_that_finds_nothing_is_not_admitted() {
    let dir = directory();
    let path = path_in(&dir);
    {
        let gateway = Scripted::new(vec![SendAnswer::Lost], vec![LookupAnswer::Lost]);
        let mut outbox = opened(&path, gateway);
        person_hold(&mut outbox, text_intent("command-1", "hello"));
        let view = outbox.send(&command("command-1")).unwrap();
        assert_eq!(
            view.outcome(),
            PhoneOutcome::Unknown(UnknownCause::LookupFailed)
        );
    }
    let gateway = Scripted::new(Vec::new(), vec![LookupAnswer::NotFound]);
    let seen = gateway.clone();
    let mut outbox = opened(&path, gateway);
    let recovered = outbox.recover().unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].outcome(),
        PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
    );
    assert!(seen.body().sent.is_empty());
    assert_eq!(seen.body().order, ["lookup"]);
    drop(outbox);
    let mut check = opened(&path, Scripted::new(Vec::new(), Vec::new()));
    assert_eq!(
        check.list().unwrap()[0].outcome(),
        PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
    );
}

#[test]
fn an_event_column_contradiction_is_corrupt() {
    let dir = directory();
    let path = path_in(&dir);
    {
        let gateway = Scripted::new(
            vec![
                SendAnswer::Receipt(Delivery::Queued),
                failure(ConversationErrorCode::TemporarilyUnavailable),
            ],
            Vec::new(),
        );
        let mut outbox = opened(&path, gateway);
        person_hold(&mut outbox, text_intent("command-1", "hello"));
        outbox.send(&command("command-1")).unwrap();
        person_hold(&mut outbox, text_intent("command-2", "hello"));
        let failed = outbox.send(&command("command-2")).unwrap();
        assert!(matches!(failed.outcome(), PhoneOutcome::Failed(_)));
    }
    let connection = Connection::open(&path).unwrap();
    let accepted = connection
        .execute(
            "UPDATE phone_intent_event SET failure = 'forbidden'
             WHERE request_id = 'command-1' AND cause = 'send_answered'",
            [],
        )
        .unwrap();
    assert_eq!(accepted, 1);
    let failed = connection
        .execute(
            "UPDATE phone_intent_event SET unknown = 'not_admitted'
             WHERE request_id = 'command-2' AND cause = 'send_answered'",
            [],
        )
        .unwrap();
    assert_eq!(failed, 1);
    drop(connection);
    let mut outbox = opened(&path, Scripted::new(Vec::new(), Vec::new()));
    assert_eq!(outbox.list().unwrap_err(), OutboxError::Corrupt);
    drop(outbox);
    let connection = Connection::open(&path).unwrap();
    let failure: String = connection
        .query_row(
            "SELECT failure FROM phone_intent_event
             WHERE request_id = 'command-1' AND cause = 'send_answered'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(failure, "forbidden");
}

#[test]
fn a_lookup_cannot_restore_running() {
    let dir = directory();
    let path = path_in(&dir);
    {
        let gateway = Scripted::new(vec![SendAnswer::Receipt(Delivery::Injected)], Vec::new());
        let mut outbox = opened(&path, gateway);
        person_hold(&mut outbox, text_intent("command-1", "hello"));
        let view = outbox.send(&command("command-1")).unwrap();
        assert_eq!(view.outcome(), PhoneOutcome::Running);
    }
    let connection = Connection::open(&path).unwrap();
    let changed = connection
        .execute(
            "UPDATE phone_intent_event SET cause = 'lookup' WHERE cause = 'send_answered'",
            [],
        )
        .unwrap();
    assert_eq!(changed, 1);
    drop(connection);
    let mut outbox = opened(&path, Scripted::new(Vec::new(), Vec::new()));
    assert_eq!(outbox.list().unwrap_err(), OutboxError::Corrupt);
    assert_eq!(
        outbox.send(&command("command-1")).unwrap_err(),
        OutboxError::Corrupt
    );
    drop(outbox);
    let connection = Connection::open(&path).unwrap();
    let outcome: String = connection
        .query_row(
            "SELECT outcome FROM phone_intent WHERE request_id = 'command-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(outcome, "running");
    let cause: String = connection
        .query_row(
            "SELECT cause FROM phone_intent_event WHERE cause = 'lookup'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(cause, "lookup");
}

#[test]
fn an_impossible_receipt_leaves_the_row() {
    let answers = [
        LookupAnswer::Found {
            stage: ReceiptStage::Ready,
            effect: None,
        },
        LookupAnswer::Found {
            stage: ReceiptStage::Settled,
            effect: None,
        },
        LookupAnswer::Found {
            stage: ReceiptStage::Settled,
            effect: Some(ReceiptEffect::Withdrawn),
        },
        LookupAnswer::Found {
            stage: ReceiptStage::Settled,
            effect: Some(ReceiptEffect::Cancelled),
        },
        LookupAnswer::Found {
            stage: ReceiptStage::Settled,
            effect: Some(ReceiptEffect::AlreadyFinal),
        },
    ];
    for answer in answers {
        let dir = directory();
        let gateway = Scripted::new(vec![SendAnswer::Lost], vec![answer]);
        let mut outbox = opened(&path_in(&dir), gateway);
        person_hold(&mut outbox, text_intent("command-1", "hello"));
        let view = outbox.send(&command("command-1")).unwrap();
        assert_eq!(
            view.outcome(),
            PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
        );
        assert_ne!(
            view.outcome(),
            PhoneOutcome::Unknown(UnknownCause::Attempted)
        );
    }
}

#[test]
fn a_transition_keeps_its_cause_and_initiator() {
    let dir = directory();
    let path = path_in(&dir);
    {
        let gateway = Scripted::new(vec![SendAnswer::Receipt(Delivery::Queued)], Vec::new());
        let mut outbox = opened(&path, gateway);
        person_hold(&mut outbox, text_intent("command-1", "hello"));
        outbox.send(&command("command-1")).unwrap();
    }
    let mut store = SqliteIntents::open(&path).unwrap();
    let events = store.events("command-1").unwrap();
    let causes: Vec<_> = events.iter().map(|event| event.cause).collect();
    assert_eq!(causes, [Cause::Held, Cause::Departed, Cause::SendAnswered]);
    assert!(events
        .iter()
        .all(|event| event.initiator == Initiator::Person));
    assert!(events
        .iter()
        .all(|event| event.observed_at_ms == 1_700_000_000_000));
    assert_eq!(events[0].outcome, PhoneOutcome::Draft);
    assert_eq!(
        events[1].outcome,
        PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
    );
    assert_eq!(events[2].outcome, PhoneOutcome::Accepted);
}

/// The second sender's lease writes SendLost, a lookup that changes the
/// outcome, and ForegroundSend. That commit rolls back. Those three events
/// are written again, and `list` still loads the row
/// (`docs/design/phone-outbox.md` S15).
#[test]
fn an_uncertain_lease_rewrites_every_transition() {
    let mut store = RollbackLease::new();
    store
        .insert(
            &PhoneRecord::held(text_intent("command-1", "hello")),
            audit(Cause::Held, PhoneOutcome::Draft),
        )
        .unwrap();
    let sends = Arc::new(AtomicUsize::new(0));
    let lookups = Arc::new(AtomicUsize::new(0));
    let (done_tx, done_rx) = mpsc::channel();
    let spawn = |store: RollbackLease,
                 sends: Arc<AtomicUsize>,
                 lookups: Arc<AtomicUsize>,
                 done_tx: mpsc::Sender<Result<PhoneOutcome, OutboxError>>| {
        thread::spawn(move || {
            let mut outbox = PhoneOutbox::new(
                store,
                CountingGateway { sends, lookups },
                SeqIds::default(),
                clock(),
            );
            let outcome = outbox
                .send(&command("command-1"))
                .map(|view| view.outcome());
            done_tx.send(outcome).unwrap();
        })
    };
    let first = spawn(
        store.clone(),
        Arc::clone(&sends),
        Arc::clone(&lookups),
        done_tx.clone(),
    );
    let second = spawn(store.clone(), sends.clone(), lookups.clone(), done_tx);
    let left = done_rx.recv_timeout(Duration::from_secs(8)).unwrap();
    let right = done_rx.recv_timeout(Duration::from_secs(8)).unwrap();
    first.join().unwrap();
    second.join().unwrap();
    let left = left.unwrap();
    let right = right.unwrap();
    let lookup_failed = PhoneOutcome::Unknown(UnknownCause::LookupFailed);
    assert!(
        (left == lookup_failed && right == PhoneOutcome::Accepted)
            || (left == PhoneOutcome::Accepted && right == lookup_failed),
        "{left:?} {right:?}"
    );
    assert_eq!(sends.load(Ordering::SeqCst), 3);
    assert_eq!(lookups.load(Ordering::SeqCst), 2);
    assert_eq!(store.uncertain_commits(), 1);

    let mut check = PhoneOutbox::new(
        store.clone(),
        Scripted::new(Vec::new(), Vec::new()),
        SeqIds::default(),
        clock(),
    );
    let views = check.list().unwrap();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].outcome(), PhoneOutcome::Accepted);
    assert_eq!(views[0].intent().text(), "hello");
    let records = store.list().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].sends(), 3);
    assert!(records[0].departed());
    let events = store.events(&command("command-1")).unwrap();
    let causes: Vec<_> = events.iter().map(|event| event.cause).collect();
    assert_eq!(
        causes,
        [
            Cause::Held,
            Cause::Departed,
            Cause::SendLost,
            Cause::Lookup,
            Cause::SendLost,
            Cause::Lookup,
            Cause::ForegroundSend,
        ]
    );
    assert_eq!(
        events[2].outcome,
        PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
    );
    assert_eq!(events[3].outcome, lookup_failed);
    assert_eq!(events[4].outcome, lookup_failed);
    assert_eq!(
        events[5].outcome,
        PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
    );
    assert_eq!(events[6].outcome, PhoneOutcome::Accepted);
}

struct CountingGateway {
    sends: Arc<AtomicUsize>,
    lookups: Arc<AtomicUsize>,
}
impl PhoneGateway for CountingGateway {
    fn send(&mut self, _: &PhoneIntent) -> SendAnswer {
        match self.sends.fetch_add(1, Ordering::SeqCst) {
            0 | 1 => SendAnswer::Lost,
            _ => SendAnswer::Receipt(Delivery::Queued),
        }
    }
    fn lookup(&mut self, _: &PhoneIntent) -> LookupAnswer {
        match self.lookups.fetch_add(1, Ordering::SeqCst) {
            0 => LookupAnswer::Lost,
            _ => LookupAnswer::NotFound,
        }
    }
}

/// Compare-and-swap rows. The second lease commit discards its writes and
/// returns [`StoreError::Uncertain`]. `list` asks the same replay as SQLite.
struct RollbackLease {
    state: Arc<Mutex<LeaseState>>,
    cv: Arc<Condvar>,
}
struct LeaseState {
    committed: Book,
    working: Option<Book>,
    leased: bool,
    draft_loads: usize,
    depart_owner: Option<ThreadId>,
    finished_leases: usize,
    uncertain: usize,
}
#[derive(Clone, Default)]
struct Book {
    rows: BTreeMap<String, PhoneRecord>,
    events: BTreeMap<String, Vec<IntentEvent>>,
}
const LEASE_WAIT: Duration = Duration::from_secs(5);

impl RollbackLease {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(LeaseState {
                committed: Book::default(),
                working: None,
                leased: false,
                draft_loads: 0,
                depart_owner: None,
                finished_leases: 0,
                uncertain: 0,
            })),
            cv: Arc::new(Condvar::new()),
        }
    }

    fn uncertain_commits(&self) -> usize {
        self.state.lock().unwrap().uncertain
    }
}
impl Clone for RollbackLease {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            cv: Arc::clone(&self.cv),
        }
    }
}
impl IntentStore for RollbackLease {
    fn load(&mut self, id: &PhoneCommandId) -> Result<Option<PhoneRecord>, StoreError> {
        let mut state = self.state.lock().unwrap();
        let record = book(&state).rows.get(id.as_str()).cloned();
        if record
            .as_ref()
            .is_some_and(|row| !row.departed() && matches!(row.outcome(), PhoneOutcome::Draft))
        {
            state.draft_loads += 1;
            self.cv.notify_all();
        }
        Ok(record)
    }

    fn insert(&mut self, record: &PhoneRecord, event: IntentEvent) -> Result<(), StoreError> {
        let mut state = self.state.lock().unwrap();
        write_insert(&mut state, record, event)
    }

    fn replace(
        &mut self,
        previous: &PhoneRecord,
        next: &PhoneRecord,
        event: IntentEvent,
    ) -> Result<(), StoreError> {
        let mut state = if !previous.departed() && next.departed() {
            wait_until(&self.state, &self.cv, |state| state.draft_loads >= 2)?
        } else {
            self.state.lock().unwrap()
        };
        write_replace(&mut state, previous, next, event)
    }

    fn replace_and_insert(
        &mut self,
        previous: &PhoneRecord,
        next: &PhoneRecord,
        draft: &PhoneRecord,
        events: &[IntentEvent],
        draft_event: IntentEvent,
    ) -> Result<(), StoreError> {
        let mut state = self.state.lock().unwrap();
        let id = next.intent().command().as_str().to_owned();
        {
            let rows = book_mut(&mut state);
            match rows.rows.get(&id) {
                Some(stored) if stored == previous => {
                    rows.events.entry(id.clone()).or_default().extend(events);
                    rows.rows.insert(id, next.clone());
                }
                _ => return Err(StoreError::Conflict),
            }
        }
        write_insert(&mut state, draft, draft_event)
    }

    fn list(&mut self) -> Result<Vec<PhoneRecord>, StoreError> {
        let state = self.state.lock().unwrap();
        let mut records: Vec<_> = state.committed.rows.values().cloned().collect();
        records.sort_by(|left, right| {
            left.intent()
                .command()
                .as_str()
                .cmp(right.intent().command().as_str())
        });
        for record in &records {
            let events = state
                .committed
                .events
                .get(record.intent().command().as_str())
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let recovery_hold = events.first().is_some_and(|event| {
                event.cause == Cause::Held && event.initiator == Initiator::Recovery
            });
            if !history_reaches(record, events, recovery_hold) {
                return Err(StoreError::Corrupt);
            }
        }
        Ok(records)
    }

    fn events(&mut self, id: &PhoneCommandId) -> Result<Vec<IntentEvent>, StoreError> {
        let state = self.state.lock().unwrap();
        Ok(book(&state)
            .events
            .get(id.as_str())
            .cloned()
            .unwrap_or_default())
    }

    fn begin_exclusive(&mut self) -> Result<(), StoreError> {
        let me = thread::current().id();
        let mut state = wait_until(&self.state, &self.cv, |state| {
            !state.leased && (state.finished_leases > 0 || state.depart_owner == Some(me))
        })?;
        state.leased = true;
        state.working = Some(state.committed.clone());
        Ok(())
    }

    fn end_exclusive(&mut self) -> Result<(), StoreError> {
        let mut state = self.state.lock().unwrap();
        if !state.leased {
            return Err(StoreError::Unavailable);
        }
        state.finished_leases += 1;
        let uncertain = state.finished_leases == 2;
        if uncertain {
            state.uncertain += 1;
            state.working = None;
        } else if let Some(working) = state.working.take() {
            state.committed = working;
        }
        state.leased = false;
        self.cv.notify_all();
        if uncertain {
            Err(StoreError::Uncertain)
        } else {
            Ok(())
        }
    }

    fn abort_exclusive(&mut self) {
        let mut state = self.state.lock().unwrap();
        if !state.leased {
            return;
        }
        state.working = None;
        state.leased = false;
        self.cv.notify_all();
    }
}

fn wait_until<'a>(
    state: &'a Mutex<LeaseState>,
    cv: &Condvar,
    mut ready: impl FnMut(&LeaseState) -> bool,
) -> Result<std::sync::MutexGuard<'a, LeaseState>, StoreError> {
    let mut guard = state.lock().unwrap();
    let start = Instant::now();
    loop {
        if ready(&guard) {
            return Ok(guard);
        }
        let left = LEASE_WAIT.saturating_sub(start.elapsed());
        if left.is_zero() {
            return Err(StoreError::Unavailable);
        }
        let (next, status) = cv.wait_timeout(guard, left).unwrap();
        guard = next;
        if status.timed_out() && !ready(&guard) {
            return Err(StoreError::Unavailable);
        }
    }
}

fn book(state: &LeaseState) -> &Book {
    state.working.as_ref().unwrap_or(&state.committed)
}

fn book_mut(state: &mut LeaseState) -> &mut Book {
    if state.leased {
        state.working.as_mut().expect("open lease")
    } else {
        &mut state.committed
    }
}

fn write_insert(
    state: &mut LeaseState,
    record: &PhoneRecord,
    event: IntentEvent,
) -> Result<(), StoreError> {
    let id = record.intent().command().as_str().to_owned();
    let rows = book_mut(state);
    if rows.rows.contains_key(&id) {
        return Err(StoreError::Conflict);
    }
    rows.events.entry(id.clone()).or_default().push(event);
    rows.rows.insert(id, record.clone());
    Ok(())
}

fn write_replace(
    state: &mut LeaseState,
    previous: &PhoneRecord,
    next: &PhoneRecord,
    event: IntentEvent,
) -> Result<(), StoreError> {
    let id = next.intent().command().as_str().to_owned();
    let leased = state.leased;
    {
        let rows = book_mut(state);
        match rows.rows.get(&id) {
            Some(stored) if stored == previous => {
                rows.events.entry(id.clone()).or_default().push(event);
                rows.rows.insert(id, next.clone());
            }
            _ => return Err(StoreError::Conflict),
        }
    }
    if !leased && !previous.departed() && next.departed() {
        state.depart_owner = Some(thread::current().id());
    }
    Ok(())
}
