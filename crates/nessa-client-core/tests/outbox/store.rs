//! The phone outbox against a real SQLite file and a scripted gateway.
use super::sqlite::{open, SqliteIntents};
use crate::outbox::application::StoreError;
use crate::outbox::application::{CommandIds, IntentStore, PhoneOutbox};
use crate::outbox::domain::{
    Cause, Delivery, Initiator, IntentError, IntentEvent, LookupAnswer, PhoneCommandId,
    PhoneFailure, PhoneImage, PhoneIntent, PhoneOutcome, PhoneRecord, ReceiptEffect, ReceiptStage,
    SendAnswer, UnknownCause, FOREGROUND_SENDS,
};
use crate::outbox::{OutboxError, PhoneGateway, PhoneView};
use nessa_auth::application::ports::Clock;
use nessa_protocol::product_contract::generated::ConversationErrorCode;
use nessa_sdk::domain::agent_execution::executions::ExecutionId;
use nessa_sdk::domain::common::value_objects::{ImageMediaType, Sha256Digest};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;

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
    uncertain_depart: bool,
    fail_answer: Option<StoreError>,
}
impl IntentStore for Memory {
    fn load(&mut self, id: &PhoneCommandId) -> Result<Option<PhoneRecord>, StoreError> {
        Ok(self.rows.get(id.as_str()).cloned())
    }
    fn insert(&mut self, record: &PhoneRecord, _event: IntentEvent) -> Result<(), StoreError> {
        self.rows.insert(
            record.intent().command().as_str().to_owned(),
            record.clone(),
        );
        Ok(())
    }
    fn replace(
        &mut self,
        previous: &PhoneRecord,
        next: &PhoneRecord,
        _event: IntentEvent,
    ) -> Result<(), StoreError> {
        if self.uncertain_depart && !previous.departed() && next.departed() {
            return Err(StoreError::Uncertain);
        }
        if let Some(error) = self.fail_answer {
            if previous.departed() && next.outcome() != previous.outcome() {
                return Err(error);
            }
        }
        self.rows
            .insert(next.intent().command().as_str().to_owned(), next.clone());
        Ok(())
    }
    fn replace_and_insert(
        &mut self,
        previous: &PhoneRecord,
        next: &PhoneRecord,
        draft: &PhoneRecord,
        event: IntentEvent,
        draft_event: IntentEvent,
    ) -> Result<(), StoreError> {
        self.replace(previous, next, event)?;
        self.insert(draft, draft_event)
    }
    fn list(&mut self) -> Result<Vec<PhoneRecord>, StoreError> {
        Ok(self.rows.values().cloned().collect())
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
    let record = PhoneRecord::held(intent).depart().unwrap();
    store
        .insert(
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
            let connection = nessa_local_database::rusqlite::Connection::open(&self.path).unwrap();
            let bit: i64 = connection
                .query_row(
                    "SELECT departed FROM phone_intent WHERE request_id = ?1",
                    nessa_local_database::rusqlite::params![intent.command().as_str()],
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
    let gateway = Scripted::new(
        vec![SendAnswer::Lost, SendAnswer::Receipt(Delivery::Queued)],
        vec![LookupAnswer::NotFound],
    );
    let seen = gateway.clone();
    let mut outbox = opened(&path_in(&dir), gateway);
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
}

#[test]
fn foreground_send_stops_at_the_bound() {
    let sends = vec![SendAnswer::Lost; usize::from(FOREGROUND_SENDS)];
    let lookups = vec![LookupAnswer::NotFound; usize::from(FOREGROUND_SENDS)];
    let dir = directory();
    let gateway = Scripted::new(sends, lookups);
    let seen = gateway.clone();
    let mut outbox = opened(&path_in(&dir), gateway);
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
            Cause::Departed,
            Cause::Lookup,
            Cause::ExplicitRetry,
            Cause::ExplicitRetry,
        ]
    );
    assert_eq!(
        events[1].outcome,
        PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
    );
    assert_eq!(
        events[2].outcome,
        PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
    );
    assert_eq!(events[3].outcome, PhoneOutcome::Accepted);
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
    let connection = nessa_local_database::rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE phone_intent SET canonical = ?1 WHERE request_id = 'command-1'",
            nessa_local_database::rusqlite::params![b"not-a-send".as_slice()],
        )
        .unwrap();
    drop(connection);
    let gateway = Scripted::new(Vec::new(), Vec::new());
    let seen = gateway.clone();
    let mut outbox = opened(&path, gateway);
    assert_eq!(outbox.list().unwrap_err(), OutboxError::Corrupt);
    assert!(seen.body().order.is_empty());
    let connection = nessa_local_database::rusqlite::Connection::open(&path).unwrap();
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
