//! The warm-up runs once, off the request path, and leaves evidence behind.
use super::{AgentWarmUp, RuntimeFingerprint, WarmUpState};
use crate::agent_warm_up::application::{
    WarmUpAudit, WarmUpAuditRecord, WarmUpError, WarmUpFuture, WarmUpRecords,
};
use crate::conversation_test_support::{Provider, ProviderFactory, TestClock};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use tokio::sync::{oneshot, Barrier};

fn runtime() -> RuntimeFingerprint {
    RuntimeFingerprint::new("/runtimes/aa/node", "/runtimes/aa/acp/index.js", "test").unwrap()
}

#[derive(Default)]
struct MemoryRecords {
    completed: Mutex<Vec<RuntimeFingerprint>>,
    reads: AtomicUsize,
    read_failure: Mutex<Option<String>>,
    write_failure: Mutex<Option<String>>,
}
impl WarmUpRecords for MemoryRecords {
    fn completed(&self, runtime: &RuntimeFingerprint) -> WarmUpFuture<'_, bool> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if let Some(failure) = self.read_failure.lock().unwrap().clone() {
            return Box::pin(async move { Err(WarmUpError::Records(failure)) });
        }
        let known = self.completed.lock().unwrap().contains(runtime);
        Box::pin(async move { Ok(known) })
    }
    fn record_completed(
        &self,
        runtime: RuntimeFingerprint,
        _observed_at_ms: u64,
    ) -> WarmUpFuture<'_, ()> {
        if let Some(failure) = self.write_failure.lock().unwrap().clone() {
            return Box::pin(async move { Err(WarmUpError::Records(failure)) });
        }
        self.completed.lock().unwrap().push(runtime);
        Box::pin(async { Ok(()) })
    }
}

#[derive(Default)]
struct RecordingAudit {
    records: Mutex<Vec<WarmUpAuditRecord>>,
    failure: Mutex<Option<String>>,
}
impl WarmUpAudit for RecordingAudit {
    fn record(&self, record: WarmUpAuditRecord) -> WarmUpFuture<'_, ()> {
        self.records.lock().unwrap().push(record);
        let failure = self.failure.lock().unwrap().clone();
        Box::pin(async move {
            match failure {
                Some(failure) => Err(WarmUpError::Audit(failure)),
                None => Ok(()),
            }
        })
    }
}

struct Fixture {
    warm_up: AgentWarmUp,
    provider: Arc<ProviderFactory>,
    records: Arc<MemoryRecords>,
    audit: Arc<RecordingAudit>,
}
fn fixture() -> Fixture {
    let provider = Arc::new(ProviderFactory::default());
    let records = Arc::new(MemoryRecords::default());
    let audit = Arc::new(RecordingAudit::default());
    Fixture {
        warm_up: AgentWarmUp::new(
            Arc::new(Provider(provider.clone())),
            Arc::new(nessa_sdk::infrastructure::session_storage::InMemoryStorage::new()),
            records.clone(),
            audit.clone(),
            Arc::new(TestClock),
            runtime(),
        ),
        provider,
        records,
        audit,
    }
}

#[tokio::test]
async fn warming_opens_and_closes_one_real_session_and_records_it() {
    let fixture = fixture();
    fixture.warm_up.wait_until_settled().await;
    assert_eq!(fixture.provider.open_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.provider.close_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.records.completed.lock().unwrap().as_slice(),
        [runtime()]
    );
    let records = fixture.audit.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.runtime, runtime());
    assert_eq!(record.before, WarmUpState::Cold);
    assert_eq!(record.after, WarmUpState::Warmed);
    assert!(record.failure.is_none());
    assert!(
        record.session_id.is_some(),
        "the session it opened is named"
    );
    assert!(!record.correlation_id.is_empty());
    assert!(record.observed_at_ms >= record.requested_at_ms);
}

#[tokio::test]
async fn an_already_warmed_runtime_is_not_launched_again() {
    let fixture = fixture();
    fixture.records.completed.lock().unwrap().push(runtime());
    fixture.warm_up.wait_until_settled().await;
    assert_eq!(fixture.provider.open_calls.load(Ordering::SeqCst), 0);
    // Nothing changed, so there is no transition to record.
    assert!(fixture.audit.records.lock().unwrap().is_empty());
}

/// The point of the whole exercise: a first message arriving while the runtime
/// is still starting waits for that launch rather than starting a second one.
#[tokio::test]
async fn a_caller_arriving_mid_warm_up_joins_it_instead_of_launching_again() {
    let fixture = fixture();
    let (release, gate) = oneshot::channel();
    *fixture.provider.open_gate.lock().unwrap() = Some(gate);
    let background = fixture.warm_up.clone();
    let started = Arc::new(Barrier::new(2));
    let waiting = tokio::spawn({
        let started = started.clone();
        async move {
            started.wait().await;
            background.wait_until_settled().await;
        }
    });
    fixture.warm_up.start();
    // The provider is inside `open` and blocked, so the warm-up is genuinely in
    // flight when the second caller arrives.
    fixture.provider.opening.notified().await;
    started.wait().await;
    let joining = tokio::spawn({
        let warm_up = fixture.warm_up.clone();
        async move { warm_up.wait_until_settled().await }
    });
    release.send(()).unwrap();
    waiting.await.unwrap();
    joining.await.unwrap();
    assert_eq!(
        fixture.provider.open_calls.load(Ordering::SeqCst),
        1,
        "one cold launch, however many callers waited on it"
    );
    assert_eq!(fixture.audit.records.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_failed_audit_prevents_a_completion_record_and_leaves_the_runtime_cold() {
    let fixture = fixture();
    *fixture.audit.failure.lock().unwrap() = Some("sink rejected".into());
    fixture.warm_up.wait_until_settled().await;
    // The session was still opened and closed: audit failure must not prevent
    // the cleanup that had already happened.
    assert_eq!(fixture.provider.open_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.provider.close_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.audit.records.lock().unwrap().len(), 1);
    // Unrecorded evidence must not leave behind a record claiming success.
    assert!(fixture.records.completed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_failed_launch_is_audited_as_still_cold_and_not_recorded_complete() {
    let fixture = fixture();
    *fixture.provider.close_failure.lock().unwrap() = Some(
        nessa_sdk::application::agent_execution::agents::AgentError::Transport(
            "pipe closed".into(),
        ),
    );
    fixture.warm_up.wait_until_settled().await;
    let records = fixture.audit.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].before, WarmUpState::Cold);
    assert_eq!(records[0].after, WarmUpState::Cold, "nothing was warmed");
    assert!(records[0].failure.is_some());
    assert!(fixture.records.completed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_unreadable_record_store_does_not_launch_or_claim_completion() {
    let fixture = fixture();
    *fixture.records.read_failure.lock().unwrap() = Some("unreadable".into());
    fixture.warm_up.wait_until_settled().await;
    assert_eq!(fixture.provider.open_calls.load(Ordering::SeqCst), 0);
    assert!(fixture.audit.records.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_failed_completion_write_keeps_its_audited_evidence() {
    let fixture = fixture();
    *fixture.records.write_failure.lock().unwrap() = Some("read-only".into());
    fixture.warm_up.wait_until_settled().await;
    let records = fixture.audit.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    // The warm-up did happen; only the record of it could not be kept, so the
    // next start pays for the launch again rather than trusting a lost write.
    assert_eq!(records[0].after, WarmUpState::Warmed);
    assert!(fixture.records.completed.lock().unwrap().is_empty());
}

/// One run per process, whoever asks. A second `wait` after the first settled
/// must not launch anything, even though nothing was recorded as complete.
#[tokio::test]
async fn the_run_is_not_repeated_after_it_has_settled() {
    let fixture = fixture();
    *fixture.records.write_failure.lock().unwrap() = Some("read-only".into());
    fixture.warm_up.wait_until_settled().await;
    fixture.warm_up.wait_until_settled().await;
    assert_eq!(fixture.provider.open_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.records.reads.load(Ordering::SeqCst), 1);
}
