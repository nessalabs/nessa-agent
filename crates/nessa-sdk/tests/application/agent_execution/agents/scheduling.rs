//! Failed queue settlement stays complete and preserves the first stop owner.
use super::*;
use crate::application::agent_execution::{
    agents::AgentFuture,
    executions::{ExecutionAudit, QueueSettlementRecord},
    providers::{
        AgentProvider, ProviderIdentity, ProviderOpenError, ProviderOpenFuture, ProviderOpenRequest,
    },
    sessions::{
        SessionManager, SessionSnapshot, SessionStorage, SessionStorageLease, StorageFuture,
    },
};
use crate::application::dto::{ModalitiesDto, ModelMetadataDto};
use crate::domain::agent_execution::{
    executions::{QueueMutation, QueueRemovalCause, SchedulingInitiator},
    prompts::{PromptText, UserMessage},
    sessions::SessionId,
};
use crate::domain::{
    effective_capabilities::value_objects::{BindingRestrictions, EffectiveCapabilities},
    model_metadata::{
        entities::ModelMetadata,
        value_objects::{Modalities, ModelFeatures},
    },
};
use crate::infrastructure::session_storage::InMemoryStorage;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
use tokio::sync::oneshot;

#[derive(Default)]
struct SettlementPanickingAudit {
    panic_once: AtomicBool,
    settlements: Mutex<Vec<QueueSettlementRecord>>,
}

struct PausingSettlementAudit {
    settlements: Mutex<Vec<QueueSettlementRecord>>,
    gate: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}

struct TestProvider;

impl AgentProvider for TestProvider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("failed-pending", "fixture", "fixture").unwrap()
    }

    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities()
    }

    fn open(&self, _request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async { Err(ProviderOpenError::no_resources(AgentError::Closed)) })
    }
}

fn capabilities() -> &'static EffectiveCapabilities {
    static CAPABILITIES: OnceLock<EffectiveCapabilities> = OnceLock::new();
    CAPABILITIES.get_or_init(|| {
        let text = ModalitiesDto {
            text: true,
            image: false,
            audio: false,
        };
        let model = ModelMetadata::try_from(ModelMetadataDto {
            provider: "anthropic".into(),
            model_id: "fixture".into(),
            display_name: "Fixture".into(),
            input: text,
            image_input: None,
            output: text,
            tool_use: true,
            reasoning: None,
            fast_mode: false,
            max_context_window_tokens: 1000,
            max_output_tokens: 100,
            knowledge_cutoff: "2026-01".into(),
            documentation_url: "https://example.com".into(),
        })
        .unwrap();
        let text = Modalities::new(true, false, false).unwrap();
        EffectiveCapabilities::new(
            &model,
            BindingRestrictions::new(
                ModelFeatures::new(text, text, true, false, false),
                model.limits(),
            ),
            model.limits(),
        )
        .unwrap()
    })
}

impl ExecutionAudit for PausingSettlementAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            if let ExecutionAuditRecord::QueueSettled(record) = record {
                self.settlements.lock().unwrap().push(record);
                let gate = self.gate.lock().unwrap().take();
                if let Some((entered, release)) = gate {
                    entered.send(()).unwrap();
                    release.await.unwrap();
                }
            }
            Ok(())
        })
    }
}

impl ExecutionAudit for SettlementPanickingAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            if let ExecutionAuditRecord::QueueSettled(record) = record {
                self.settlements.lock().unwrap().push(record);
                if !self.panic_once.swap(true, Ordering::SeqCst) {
                    panic!("first queue settlement audit panicked");
                }
            }
            Ok(())
        })
    }
}

struct PanickingQueueStorage {
    inner: InMemoryStorage,
    mutation: Arc<Mutex<Option<QueueMutation>>>,
}

struct PanickingQueueLease {
    inner: Box<dyn SessionStorageLease>,
    mutation: Arc<Mutex<Option<QueueMutation>>>,
}

impl SessionStorage for PanickingQueueStorage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            Ok(Box::new(PanickingQueueLease {
                inner: self.inner.open(id).await?,
                mutation: self.mutation.clone(),
            }) as Box<dyn SessionStorageLease>)
        })
    }
}

impl SessionStorageLease for PanickingQueueLease {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        self.inner.load()
    }

    fn save(&self, snapshot: SessionSnapshot) -> StorageFuture<'_, ()> {
        let should_panic = self
            .mutation
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|mutation| {
                snapshot
                    .queue_history
                    .last()
                    .is_some_and(|entry| &entry.mutation == mutation)
            });
        if should_panic {
            self.mutation.lock().unwrap().take();
            return Box::pin(async { panic!("queue membership persistence panic") });
        }
        self.inner.save(snapshot)
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        self.inner.erase()
    }
}

async fn prepared(storage: Arc<dyn SessionStorage>, audit: Arc<dyn ExecutionAudit>) -> Agent {
    let manager = SessionManager::open(
        None,
        storage,
        std::sync::Arc::new(
            nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
        ),
    )
    .await
    .unwrap();
    Agent::prepare(Arc::new(TestProvider), manager, audit)
        .await
        .unwrap()
}

fn request(id: &str) -> ExecutionRequest {
    ExecutionRequest {
        execution_id: ExecutionId::new(id).unwrap(),
        user_message: UserMessage::text_only(PromptText::new("queued input").unwrap()),
        estimated_input_tokens: 2,
        reserved_output_tokens: 10,
    }
}

fn actor() -> ActionContext {
    ActionContext::new("caller", "test", "failed-pending").unwrap()
}

#[tokio::test]
async fn failed_attachment_settlement_panics_do_not_discard_the_queue_tail() {
    let mutation = Arc::new(Mutex::new(None));
    let storage = Arc::new(PanickingQueueStorage {
        inner: InMemoryStorage::new(),
        mutation: mutation.clone(),
    });
    let audit = Arc::new(SettlementPanickingAudit::default());
    let agent = prepared(storage, audit.clone()).await;
    let _invocation = agent.inner.invocation.lock().await;
    let first_id = ExecutionId::new("failed-attachment-first").unwrap();
    let first = agent
        .enqueue(request(first_id.as_str()), actor())
        .await
        .unwrap();
    let second = agent
        .enqueue(request("failed-attachment-second"), actor())
        .await
        .unwrap();
    mutation.lock().unwrap().replace(QueueMutation::Removed {
        id: first_id,
        cause: QueueRemovalCause::DispatchFailed,
    });
    let failure = AgentError::Protocol("automatic attachment failed".into());
    let mut scheduler = agent.inner.scheduler.lock().await;
    let aggregate = agent
        .settle_failed_pending(&mut scheduler, failure.clone())
        .await
        .unwrap_err();
    drop(scheduler);

    let first = first.wait().await;
    let second = second.wait().await;
    assert!(format!("{first:?}").contains("automatic attachment failed"));
    assert!(format!("{first:?}").contains("AuditFailure"));
    assert!(format!("{first:?}").contains("persistence panicked"));
    assert_eq!(second, Err(failure));
    assert!(format!("{aggregate:?}").contains("AuditFailure"));
    assert!(format!("{aggregate:?}").contains("persistence panicked"));
    assert_eq!(audit.settlements.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn automatic_failure_claim_precedes_close_while_settlement_audit_waits() {
    let (entered, waiting) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let audit = Arc::new(PausingSettlementAudit {
        settlements: Mutex::new(Vec::new()),
        gate: Mutex::new(Some((entered, released))),
    });
    let agent = prepared(Arc::new(InMemoryStorage::new()), audit.clone()).await;
    let _invocation = agent.inner.invocation.lock().await;
    let queued = agent
        .enqueue(request("failure-before-close"), actor())
        .await
        .unwrap();
    let failure = AgentError::Protocol("automatic attachment failed".into());
    let settlement = tokio::spawn({
        let agent = agent.clone();
        let failure = failure.clone();
        async move {
            let mut scheduler = agent.inner.scheduler.lock().await;
            agent.settle_failed_pending(&mut scheduler, failure).await
        }
    });
    waiting.await.unwrap();
    let closer = ActionContext::new("closer", "test", "close-after-claim").unwrap();
    let attempt = agent.start_shutdown(SessionCloseRequest::Explicit(closer));
    release.send(()).unwrap();

    assert_eq!(settlement.await.unwrap(), Ok(()));
    assert_eq!(queued.wait().await, Err(failure));
    {
        let records = audit.settlements.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].cause(), SchedulingCause::DispatchFailed);
        assert_eq!(records[0].initiator(), SchedulingInitiator::Automatic);
        assert_eq!(records[0].submitted_by(), &actor());
        assert_eq!(records[0].initiated_by(), None);
    }
    agent.inner.lifecycle.complete_stop(&attempt).await;
}

#[tokio::test]
async fn explicit_close_precedes_unclaimed_automatic_failure_settlement() {
    let audit = Arc::new(PausingSettlementAudit {
        settlements: Mutex::new(Vec::new()),
        gate: Mutex::new(None),
    });
    let agent = prepared(Arc::new(InMemoryStorage::new()), audit.clone()).await;
    let _invocation = agent.inner.invocation.lock().await;
    let submitted_by = actor();
    let queued = agent
        .enqueue(request("close-before-failure"), submitted_by.clone())
        .await
        .unwrap();
    let closer = ActionContext::new("closer", "test", "close-before-claim").unwrap();
    let attempt = agent.start_shutdown(SessionCloseRequest::Explicit(closer.clone()));
    let mut scheduler = agent.inner.scheduler.lock().await;
    assert_eq!(
        agent
            .settle_failed_pending(
                &mut scheduler,
                AgentError::Protocol("later automatic attachment failure".into()),
            )
            .await,
        Ok(())
    );
    drop(scheduler);

    assert_eq!(queued.wait().await, Err(AgentError::Closed));
    {
        let records = audit.settlements.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].cause(), SchedulingCause::SessionClosed);
        assert_eq!(records[0].initiator(), SchedulingInitiator::Caller);
        assert_eq!(records[0].submitted_by(), &submitted_by);
        assert_eq!(records[0].initiated_by(), Some(&closer));
    }
    agent.inner.lifecycle.complete_stop(&attempt).await;
}

// The runner tests below drive `run_queue` on the test's own task rather than
// spawning it. One poll runs it to its first wait, and the test asserts that
// poll was `Pending`, so "the runner is waiting" is checked, not assumed from
// the order in which the runtime polls tasks (which Tokio does not promise).
// Setting `running` first, as `start_runner` would, keeps admission from
// spawning a second runner. Hand-over of the invocation slot relies on Tokio's
// `Mutex` granting waiters in the order they called `lock`, which it documents.
async fn poll_once<F: std::future::Future + Unpin>(future: &mut F) -> std::task::Poll<F::Output> {
    std::future::poll_fn(|context| {
        std::task::Poll::Ready(std::pin::Pin::new(&mut *future).poll(context))
    })
    .await
}

// Row one of the orderings table in docs/agent_execution/scheduling.md. An
// attachment starts the runner whether or not anything is queued. With the
// scheduler lock held here, the runner stops at its first wait. A runner that
// took the invocation slot before finding the queue empty turned this direct
// invoke into Busy, which is how `dispatch_save_panic_does_not_inherit_previous_close_actor`
// hung (#366). Unattached, the invoke must instead reach the lifecycle's own
// refusal.
#[tokio::test]
async fn an_idle_queue_runner_leaves_the_invocation_slot_to_a_direct_invoke() {
    // No queued input reaches settlement here, so this audit never panics.
    let audit = Arc::new(SettlementPanickingAudit::default());
    let agent = prepared(Arc::new(InMemoryStorage::new()), audit).await;
    let bound = std::time::Duration::from_secs(5);
    let mut scheduler = agent.inner.scheduler.lock().await;
    scheduler.running = true;
    let mut runner = Box::pin(agent.run_queue());
    assert!(poll_once(&mut runner).await.is_pending());

    let invoked = tokio::time::timeout(
        bound,
        agent.invoke(request("direct-beside-idle-runner"), actor()),
    )
    .await
    .expect("direct invoke stalled beside an idle runner");
    assert!(
        matches!(invoked, Err(AgentError::AttachmentUnavailable(_))),
        "{invoked:?}"
    );

    drop(scheduler);
    tokio::time::timeout(bound, runner)
        .await
        .expect("idle runner did not exit");
    assert!(!agent.inner.scheduler.lock().await.running);
    assert!(agent.inner.invocation.try_lock().is_ok());
}

// Row three. While a direct invocation holds the slot, so that no runner can
// finish, an admission while no runner is running starts one.
#[tokio::test]
async fn an_admission_while_no_runner_is_running_starts_one() {
    let audit = Arc::new(PausingSettlementAudit {
        settlements: Mutex::new(Vec::new()),
        gate: Mutex::new(None),
    });
    let agent = prepared(Arc::new(InMemoryStorage::new()), audit).await;
    let _direct = agent.inner.invocation.clone().lock_owned().await;
    assert!(!agent.inner.scheduler.lock().await.running);
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        agent.enqueue(request("admitted-with-no-runner"), actor()),
    )
    .await
    .expect("admission stalled")
    .unwrap();
    assert!(
        agent.inner.scheduler.lock().await.running,
        "an admission while no runner was running did not start one"
    );
}

// Row six by removal, and the waiting half of row two (its selection is the
// integration test's). While a direct invocation holds the slot, the runner
// passes the pre-check and waits for it. Removing the input empties the queue
// under it. When the slot is released the runner owns it until it is polled,
// so a direct `invoke` in that window is Busy; once the runner has seen the
// empty queue it exits and releases the slot.
#[tokio::test]
async fn a_runner_whose_queue_empties_while_it_waits_releases_the_slot_and_stops() {
    let audit = Arc::new(PausingSettlementAudit {
        settlements: Mutex::new(Vec::new()),
        gate: Mutex::new(None),
    });
    let agent = prepared(Arc::new(InMemoryStorage::new()), audit).await;
    let bound = std::time::Duration::from_secs(5);
    let direct = agent.inner.invocation.clone().lock_owned().await;
    agent.inner.scheduler.lock().await.running = true;
    let removed_id = ExecutionId::new("removed-while-the-runner-waits").unwrap();
    let removed = tokio::time::timeout(bound, agent.enqueue(request(removed_id.as_str()), actor()))
        .await
        .expect("admission stalled")
        .unwrap();
    let mut runner = Box::pin(agent.run_queue());
    assert!(
        poll_once(&mut runner).await.is_pending(),
        "the runner did not wait for the slot"
    );

    assert_eq!(
        tokio::time::timeout(bound, agent.remove_queued(removed_id, actor()))
            .await
            .expect("removal stalled"),
        Ok(QueueRemoval::Removed)
    );
    assert_eq!(
        tokio::time::timeout(bound, removed.wait())
            .await
            .expect("the removed receipt did not settle"),
        Err(AgentError::Closed)
    );

    drop(direct);
    assert_eq!(
        agent
            .invoke(request("direct-before-the-runner-exits"), actor())
            .await,
        Err(AgentError::Busy),
        "the released slot went to the waiting runner"
    );
    tokio::time::timeout(bound, runner)
        .await
        .expect("the runner did not exit after its queue emptied");
    assert!(!agent.inner.scheduler.lock().await.running);
    assert!(matches!(
        agent
            .invoke(request("direct-after-the-runner-exits"), actor())
            .await,
        Err(AgentError::AttachmentUnavailable(_))
    ));
}

// Explicit close stops every permit, then drains the queue before it settles
// each owner (`close_scheduled`, `cancel_pending`). A panic that escapes that
// settlement leaves stopped owners in `pending` with no queue entry, and a
// later attachment starts a runner. Strand one by hand the same way. The first
// runner passed the pre-check while the entry was queued, so it waits for the
// slot and then exits past the owner (row six). A runner started with only the
// unstopped owner still passes the pre-check and takes the slot, then passes
// over it and exits (row four). Once it is stopped, a new runner must take the
// slot and settle it (row five), not exit because the queue is empty.
#[tokio::test]
async fn a_runner_settles_a_stopped_owner_that_is_no_longer_queued() {
    let audit = Arc::new(PausingSettlementAudit {
        settlements: Mutex::new(Vec::new()),
        gate: Mutex::new(None),
    });
    let agent = prepared(Arc::new(InMemoryStorage::new()), audit).await;
    let bound = std::time::Duration::from_secs(5);
    let direct = agent.inner.invocation.clone().lock_owned().await;
    agent.inner.scheduler.lock().await.running = true;
    let stranded_id = ExecutionId::new("owner-without-a-queue-entry").unwrap();
    let stranded =
        tokio::time::timeout(bound, agent.enqueue(request(stranded_id.as_str()), actor()))
            .await
            .expect("admission stalled")
            .unwrap();
    let mut runner = Box::pin(agent.run_queue());
    assert!(poll_once(&mut runner).await.is_pending());
    assert!(agent
        .inner
        .scheduler
        .lock()
        .await
        .queue
        .remove(&stranded_id)
        .is_some());
    drop(direct);
    assert_eq!(
        agent
            .invoke(request("direct-beside-the-stranded-owner"), actor())
            .await,
        Err(AgentError::Busy),
        "the released slot went to the runner the stranded owner keeps"
    );
    tokio::time::timeout(bound, runner)
        .await
        .expect("the first runner did not exit");
    assert!(agent
        .inner
        .scheduler
        .lock()
        .await
        .pending
        .contains_key(&stranded_id));

    // Row four: a runner that starts with only that unstopped owner passes the
    // pre-check and waits for the slot, then takes it, passes over the owner
    // and exits, leaving it pending.
    let direct = agent.inner.invocation.clone().lock_owned().await;
    agent.inner.scheduler.lock().await.running = true;
    let mut runner = Box::pin(agent.run_queue());
    assert!(
        poll_once(&mut runner).await.is_pending(),
        "a runner with only an unstopped owner did not wait for the slot"
    );
    drop(direct);
    assert_eq!(
        agent
            .invoke(request("direct-beside-an-unstopped-owner"), actor())
            .await,
        Err(AgentError::Busy),
        "a runner with only an unstopped owner did not take the slot"
    );
    tokio::time::timeout(bound, runner)
        .await
        .expect("the runner did not exit past the unstopped owner");
    assert!(agent
        .inner
        .scheduler
        .lock()
        .await
        .pending
        .contains_key(&stranded_id));

    // Row five: once stopped, a new runner takes the slot and settles it.
    let attempt = agent.start_shutdown(SessionCloseRequest::Explicit(actor()));
    let direct = agent.inner.invocation.clone().lock_owned().await;
    {
        let mut scheduler = agent.inner.scheduler.lock().await;
        assert!(scheduler.pending[&stranded_id]
            ._work
            .cancellation()
            .is_some());
        scheduler.running = true;
    }
    let mut runner = Box::pin(agent.run_queue());
    assert!(
        poll_once(&mut runner).await.is_pending(),
        "a runner with a stopped owner to settle did not wait for the slot"
    );
    drop(direct);
    assert_eq!(
        agent
            .invoke(request("direct-beside-a-stopped-owner"), actor())
            .await,
        Err(AgentError::Busy),
        "a runner with a stopped owner to settle did not take the slot"
    );
    tokio::time::timeout(bound, runner)
        .await
        .expect("the runner did not exit after settling the stopped owner");
    assert!(tokio::time::timeout(bound, stranded.wait())
        .await
        .expect("the stopped owner was never settled")
        .is_err());
    assert!(!agent
        .inner
        .scheduler
        .lock()
        .await
        .pending
        .contains_key(&stranded_id));
    agent.inner.lifecycle.complete_stop(&attempt).await;
}

// Row six, by close. While a direct invocation holds the slot, the runner
// passes the pre-check and waits for it. Close then settles the queued input
// and waits for the slot behind the runner. When the slot is released the
// runner owns it first, so a direct `invoke` is Busy until the runner has found
// the lifecycle closed and exited. Close then takes the slot and completes.
#[tokio::test]
async fn close_while_a_runner_waits_leaves_it_nothing_to_run_and_both_finish() {
    let audit = Arc::new(PausingSettlementAudit {
        settlements: Mutex::new(Vec::new()),
        gate: Mutex::new(None),
    });
    let agent = prepared(Arc::new(InMemoryStorage::new()), audit).await;
    let bound = std::time::Duration::from_secs(5);
    let direct = agent.inner.invocation.clone().lock_owned().await;
    agent.inner.scheduler.lock().await.running = true;
    let closed = tokio::time::timeout(
        bound,
        agent.enqueue(request("closed-while-the-runner-waits"), actor()),
    )
    .await
    .expect("admission stalled")
    .unwrap();
    let mut runner = Box::pin(agent.run_queue());
    assert!(
        poll_once(&mut runner).await.is_pending(),
        "the runner did not wait for the slot"
    );

    let closing = tokio::spawn({
        let agent = agent.clone();
        async move { agent.close(actor()).await }
    });
    assert_eq!(
        tokio::time::timeout(bound, closed.wait())
            .await
            .expect("close did not settle the queued receipt"),
        Err(AgentError::Closed)
    );
    assert!(agent.inner.scheduler.lock().await.queue.is_empty());

    drop(direct);
    assert_eq!(
        agent
            .invoke(request("direct-while-the-runner-holds-the-slot"), actor())
            .await,
        Err(AgentError::Busy),
        "the released slot went to the waiting runner"
    );
    tokio::time::timeout(bound, runner)
        .await
        .expect("the runner did not exit after close emptied its queue");
    assert!(
        !agent.inner.scheduler.lock().await.running,
        "the runner exited past the closed lifecycle without clearing running"
    );
    let outcome = tokio::time::timeout(bound, closing)
        .await
        .expect("close stalled behind the runner")
        .expect("close task panicked");
    assert!(outcome.is_ok(), "{outcome:?}");
}
