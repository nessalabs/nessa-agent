//! The composition-owned lane around dynamic OpenCode preparation.

use super::{CurrentOpenCodeWarmUp, CurrentWarmUpAdmission};
use crate::{
    agent_warm_up::{
        application::{
            WarmUpAudit, WarmUpAuditRecord, WarmUpError, WarmUpFuture, WarmUpLaunchOwnership,
            WarmUpRecords,
        },
        domain::RuntimeFingerprint,
    },
    conversation::application::ConversationAgent,
    conversation_test_support::{AcceptingAudit, Provider, ProviderFactory, TestClock},
};
use nessa_sdk::{
    application::agent_execution::providers::{
        AgentProvider, CleanupFuture, CleanupReport, ProviderCleanup, ProviderIdentity,
        ProviderOpenError, ProviderOpenFuture, ProviderOpenRequest,
    },
    domain::effective_capabilities::value_objects::EffectiveCapabilities,
};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use tokio::sync::oneshot;

#[derive(Default)]
struct MemoryRecords {
    completed: Mutex<Vec<RuntimeFingerprint>>,
    reject_writes: AtomicBool,
}

impl WarmUpRecords for MemoryRecords {
    fn completed(&self, runtime: &RuntimeFingerprint) -> WarmUpFuture<'_, bool> {
        let completed = self.completed.lock().unwrap().contains(runtime);
        Box::pin(async move { Ok(completed) })
    }

    fn record_completed(
        &self,
        runtime: RuntimeFingerprint,
        _observed_at_ms: u64,
    ) -> WarmUpFuture<'_, ()> {
        if self.reject_writes.load(Ordering::SeqCst) {
            return Box::pin(async { Err(WarmUpError::Records("write rejected".into())) });
        }
        self.completed.lock().unwrap().push(runtime);
        Box::pin(async { Ok(()) })
    }
}

#[derive(Default)]
struct RecordingAudit {
    records: Mutex<Vec<WarmUpAuditRecord>>,
    reject: AtomicBool,
}

impl WarmUpAudit for RecordingAudit {
    fn record(&self, record: WarmUpAuditRecord) -> WarmUpFuture<'_, ()> {
        self.records.lock().unwrap().push(record);
        let reject = self.reject.load(Ordering::SeqCst);
        Box::pin(async move {
            if reject {
                Err(WarmUpError::Audit("rejected".into()))
            } else {
                Ok(())
            }
        })
    }
}

struct IdentifiedProvider {
    inner: Provider,
    model: &'static str,
}

impl AgentProvider for IdentifiedProvider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("opencode", self.model, self.model).unwrap()
    }

    fn capabilities(&self) -> &EffectiveCapabilities {
        self.inner.capabilities()
    }

    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        self.inner.open(request)
    }
}

struct RetainedOpenProvider {
    inner: IdentifiedProvider,
    cleanup: Arc<RetainedCleanup>,
}

struct RetainedCleanup {
    calls: AtomicUsize,
}

impl ProviderCleanup for RetainedCleanup {
    fn retry_cleanup(&self) -> CleanupFuture<'_> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            CleanupReport::unconfirmed(
                nessa_sdk::application::agent_execution::agents::AgentError::CleanupUncertain,
            )
        })
    }
}

impl AgentProvider for RetainedOpenProvider {
    fn identity(&self) -> ProviderIdentity {
        self.inner.identity()
    }

    fn capabilities(&self) -> &EffectiveCapabilities {
        self.inner.capabilities()
    }

    fn open(&self, _request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async move {
            Err(ProviderOpenError::with_cleanup(
                nessa_sdk::application::agent_execution::agents::AgentError::Protocol(
                    "open failed".into(),
                ),
                self.cleanup.clone(),
            ))
        })
    }
}

fn agent(factory: Arc<ProviderFactory>, model: &'static str) -> ConversationAgent {
    ConversationAgent {
        provider: Arc::new(IdentifiedProvider {
            inner: Provider::new(factory),
            model,
        }),
        execution_audit: Arc::new(AcceptingAudit),
        reserved_output_tokens: 4096,
        readiness: None,
    }
}

fn coordinator(records: Arc<MemoryRecords>, audit: Arc<RecordingAudit>) -> CurrentOpenCodeWarmUp {
    CurrentOpenCodeWarmUp::new(records, audit, Arc::new(TestClock))
}

#[tokio::test]
async fn same_fingerprint_joins_and_different_fingerprint_reobserves_after_release() {
    let records = Arc::new(MemoryRecords::default());
    let lane = coordinator(records, Arc::new(RecordingAudit::default()));
    let first_factory = Arc::new(ProviderFactory::default());
    let second_factory = Arc::new(ProviderFactory::default());
    let third_factory = Arc::new(ProviderFactory::default());
    let first = agent(first_factory.clone(), "first");
    let same = agent(first_factory.clone(), "first");
    let second = agent(second_factory.clone(), "second");
    let third = agent(third_factory.clone(), "third");
    let (release, gate) = oneshot::channel();
    *first_factory.open_gate.lock().unwrap() = Some(gate);

    let CurrentWarmUpAdmission::Prepared(first_preparation) = lane.admit(&first).unwrap() else {
        panic!("an empty lane admits the first runtime")
    };
    let CurrentWarmUpAdmission::Prepared(same_preparation) = lane.admit(&same).unwrap() else {
        panic!("the same fingerprint joins")
    };
    first_factory.opening.notified().await;
    assert_eq!(first_factory.open_calls.load(Ordering::SeqCst), 1);
    let CurrentWarmUpAdmission::Reobserve(wait) = lane.admit(&second).unwrap() else {
        panic!("a different fingerprint cannot join")
    };
    let CurrentWarmUpAdmission::Reobserve(rapid_wait) = lane.admit(&third).unwrap() else {
        panic!("a rapid later fingerprint also waits without being retained")
    };
    assert_eq!(second_factory.open_calls.load(Ordering::SeqCst), 0);
    let abandoned = tokio::spawn(wait.released());
    let rapid_released = tokio::spawn(rapid_wait.released());
    abandoned.abort();
    let _ = abandoned.await;

    release.send(()).unwrap();
    let first_terminal = first_preparation.0.wait_for_terminal().await;
    let same_terminal = same_preparation.0.wait_for_terminal().await;
    assert!(Arc::ptr_eq(&first_terminal, &same_terminal));
    rapid_released.await.unwrap();

    let CurrentWarmUpAdmission::Prepared(third_preparation) = lane.admit(&third).unwrap() else {
        panic!("the caller reobserves and admits the current fingerprint")
    };
    third_preparation.0.wait_for_terminal().await;
    assert_eq!(third_factory.open_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        second_factory.open_calls.load(Ordering::SeqCst),
        0,
        "the intermediate candidate was dropped rather than queued"
    );
}

#[tokio::test]
async fn released_audit_or_record_failure_allows_a_fresh_successor() {
    for reject_audit in [true, false] {
        let records = Arc::new(MemoryRecords::default());
        records.reject_writes.store(!reject_audit, Ordering::SeqCst);
        let audit = Arc::new(RecordingAudit::default());
        audit.reject.store(reject_audit, Ordering::SeqCst);
        let lane = coordinator(records, audit);
        let first = agent(Arc::new(ProviderFactory::default()), "first");
        let second_factory = Arc::new(ProviderFactory::default());
        let second = agent(second_factory.clone(), "second");

        let CurrentWarmUpAdmission::Prepared(first_preparation) = lane.admit(&first).unwrap()
        else {
            panic!("first runtime admitted")
        };
        let CurrentWarmUpAdmission::Reobserve(wait) = lane.admit(&second).unwrap() else {
            panic!("second runtime waits")
        };
        assert_eq!(
            first_preparation
                .0
                .wait_for_terminal()
                .await
                .launch_ownership(),
            WarmUpLaunchOwnership::Released
        );
        wait.released().await;
        let CurrentWarmUpAdmission::Prepared(second_preparation) = lane.admit(&second).unwrap()
        else {
            panic!("released failures do not fence a successor")
        };
        second_preparation.0.wait_for_terminal().await;
        assert_eq!(second_factory.open_calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn uncertain_cleanup_retains_the_lane_without_starting_a_waiting_fingerprint() {
    let audit = Arc::new(RecordingAudit::default());
    audit.reject.store(true, Ordering::SeqCst);
    let lane = coordinator(Arc::new(MemoryRecords::default()), audit);
    let first_factory = Arc::new(ProviderFactory::default());
    let cleanup = Arc::new(RetainedCleanup {
        calls: AtomicUsize::new(0),
    });
    let first = ConversationAgent {
        provider: Arc::new(RetainedOpenProvider {
            inner: IdentifiedProvider {
                inner: Provider::new(first_factory),
                model: "first",
            },
            cleanup: cleanup.clone(),
        }),
        execution_audit: Arc::new(AcceptingAudit),
        reserved_output_tokens: 4096,
        readiness: None,
    };
    let second_factory = Arc::new(ProviderFactory::default());
    let second = agent(second_factory.clone(), "second");

    let CurrentWarmUpAdmission::Prepared(first_preparation) = lane.admit(&first).unwrap() else {
        panic!("first runtime admitted")
    };
    assert_eq!(
        first_preparation
            .0
            .wait_for_terminal()
            .await
            .launch_ownership(),
        WarmUpLaunchOwnership::Retained
    );
    let CurrentWarmUpAdmission::Reobserve(wait) = lane.admit(&second).unwrap() else {
        panic!("uncertain ownership fences a different runtime")
    };
    assert!(
        tokio::time::timeout(std::time::Duration::ZERO, wait.released())
            .await
            .is_err()
    );
    assert_eq!(second_factory.open_calls.load(Ordering::SeqCst), 0);
    assert_eq!(cleanup.calls.load(Ordering::SeqCst), 2);
}
