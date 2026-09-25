//! Joins the conversation context's readiness port to the warm-up that
//! satisfies it. The two contexts stay independent: the conversation owns the
//! port, the warm-up knows nothing about conversations, and composition is the
//! only place that knows both.
use crate::{
    agent_warm_up::{
        application::{AgentWarmUp, WarmUpAudit, WarmUpLaunchOwnership, WarmUpRecords},
        domain::RuntimeFingerprint,
    },
    conversation::application::{ConversationAgent, ConversationError, RuntimeReadiness},
};
use nessa_auth::application::ports::Clock;
use nessa_sdk::infrastructure::session_storage::InMemoryStorage;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::watch;

#[derive(Clone)]
pub(super) struct PreparedRuntime(pub(super) AgentWarmUp);

impl RuntimeReadiness for PreparedRuntime {
    fn wait(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(self.0.wait_until_settled())
    }
}

/// Composition's single lane for automatic OpenCode preparation.
///
/// The lane retains no queued provider or credential. A caller for a different
/// fingerprint waits for the active run to confirm release, then resolves the
/// current OpenCode observation again. An uncertain run stays in the lane for
/// the process lifetime, conservatively retaining its process/use owner.
#[derive(Clone)]
pub(super) struct CurrentOpenCodeWarmUp {
    inner: Arc<CurrentOpenCodeWarmUpInner>,
}

struct CurrentOpenCodeWarmUpInner {
    state: Mutex<Option<ActiveWarmUp>>,
    changed: watch::Sender<u64>,
    next_id: AtomicU64,
    records: Arc<dyn WarmUpRecords>,
    audit: Arc<dyn WarmUpAudit>,
    clock: Arc<dyn Clock>,
}

struct ActiveWarmUp {
    id: u64,
    runtime: RuntimeFingerprint,
    warm_up: AgentWarmUp,
}

pub(super) enum CurrentWarmUpAdmission {
    Prepared(PreparedRuntime),
    Reobserve(CurrentWarmUpWait),
}

pub(super) struct CurrentWarmUpWait {
    changed: watch::Receiver<u64>,
}

impl CurrentWarmUpWait {
    /// Wait until the active run confirms release. A retained run deliberately
    /// never advances this receiver; the surrounding cold-slot deadline and
    /// stop signal remain the caller's bounded cancellation owners.
    pub(super) async fn released(mut self) {
        let _ = self.changed.changed().await;
    }
}

impl CurrentOpenCodeWarmUp {
    pub(super) fn new(
        records: Arc<dyn WarmUpRecords>,
        audit: Arc<dyn WarmUpAudit>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            inner: Arc::new(CurrentOpenCodeWarmUpInner {
                state: Mutex::new(None),
                changed: watch::channel(0).0,
                next_id: AtomicU64::new(1),
                records,
                audit,
                clock,
            }),
        }
    }

    /// Admit or join automatic preparation for the resolved provider.
    ///
    /// This method performs no provider effect while holding the lane lock.
    /// A different fingerprint is not retained: its caller receives only a
    /// release waiter and must discard the candidate before waiting.
    pub(super) fn admit(
        &self,
        agent: &ConversationAgent,
    ) -> Result<CurrentWarmUpAdmission, ConversationError> {
        let identity = agent.provider.identity();
        let runtime =
            RuntimeFingerprint::new(identity.name(), identity.model_id(), identity.context())
                .map_err(|_| ConversationError::Unavailable)?;
        let (warm_up, id, created) = {
            let mut state = self.inner.state.lock().unwrap();
            if let Some(active) = state.as_ref() {
                if active.runtime == runtime {
                    (active.warm_up.clone(), active.id, false)
                } else {
                    return Ok(CurrentWarmUpAdmission::Reobserve(CurrentWarmUpWait {
                        changed: self.inner.changed.subscribe(),
                    }));
                }
            } else {
                let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
                let warm_up = AgentWarmUp::new(
                    agent.provider.clone(),
                    agent.execution_audit.clone(),
                    Arc::new(InMemoryStorage::new()),
                    self.inner.records.clone(),
                    self.inner.audit.clone(),
                    self.inner.clock.clone(),
                    runtime.clone(),
                );
                *state = Some(ActiveWarmUp {
                    id,
                    runtime,
                    warm_up: warm_up.clone(),
                });
                (warm_up, id, true)
            }
        };
        if created {
            warm_up.start();
            let owner = self.clone();
            let observing = warm_up.clone();
            tokio::spawn(async move {
                let terminal = observing.wait_for_terminal().await;
                if terminal.launch_ownership() == WarmUpLaunchOwnership::Released {
                    owner.release(id);
                }
            });
        }
        Ok(CurrentWarmUpAdmission::Prepared(PreparedRuntime(warm_up)))
    }

    fn release(&self, id: u64) {
        let released = {
            let mut state = self.inner.state.lock().unwrap();
            if state.as_ref().is_some_and(|active| active.id == id) {
                *state = None;
                true
            } else {
                false
            }
        };
        if released {
            self.inner.changed.send_modify(|revision| *revision += 1);
        }
    }
}

#[cfg(test)]
#[path = "../../tests/conversation/prepared_runtime.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/composition/current_warm_up.rs"]
mod coordinator_tests;
