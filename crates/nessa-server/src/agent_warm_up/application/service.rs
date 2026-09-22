use super::ports::{ProviderFailure, WarmUpAudit, WarmUpAuditRecord, WarmUpError, WarmUpRecords};
use crate::agent_warm_up::domain::{RuntimeFingerprint, WarmUpCause, WarmUpState};
use nessa_auth::application::ports::Clock;
use nessa_sdk::application::agent_execution::{
    agents::{Agent, AgentError, AttachmentFailureCode, AttachmentPhase, AttachmentRequest},
    executions::ExecutionAudit,
    permissions::ActionContext,
    providers::AgentProvider,
    sessions::{SessionManager, SessionStorage},
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::watch;
use uuid::Uuid;

/// Surface recorded as the initiator of a warm-up. The gateway acts as its own
/// principal here; no human asked for this and none is claimed.
const WARM_UP_SURFACE: &str = "runtime_warm_up";
const GATEWAY_PRINCIPAL: &str = "gateway";

/// Runs the configured runtime through a full open and close, once, so the
/// operating system's first-execution scan is paid before anyone is waiting.
///
/// Cloning shares the single run. Whoever asks first starts it, everyone else
/// waits for that one — a first message arriving mid-warm-up must not start a
/// second cold launch.
///
/// The run belongs to a task of its own rather than to whoever triggered it,
/// because the conversation that waits here is cancellable by design: a caller
/// giving up must not abandon a launch half-finished, nor let the next caller
/// start a second one.
#[derive(Clone)]
pub struct AgentWarmUp {
    inner: Arc<Inner>,
}
struct Inner {
    provider: Arc<dyn AgentProvider>,
    execution_audit: Arc<dyn ExecutionAudit>,
    storage: Arc<dyn SessionStorage>,
    records: Arc<dyn WarmUpRecords>,
    audit: Arc<dyn WarmUpAudit>,
    clock: Arc<dyn Clock>,
    runtime: RuntimeFingerprint,
    started: AtomicBool,
    settled: watch::Sender<bool>,
}

impl AgentWarmUp {
    /// Prepare a warm-up for one configured runtime. Nothing runs until
    /// [`Self::start`] or a waiting caller asks for it.
    pub fn new(
        provider: Arc<dyn AgentProvider>,
        execution_audit: Arc<dyn ExecutionAudit>,
        storage: Arc<dyn SessionStorage>,
        records: Arc<dyn WarmUpRecords>,
        audit: Arc<dyn WarmUpAudit>,
        clock: Arc<dyn Clock>,
        runtime: RuntimeFingerprint,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                provider,
                execution_audit,
                storage,
                records,
                audit,
                clock,
                runtime,
                started: AtomicBool::new(false),
                settled: watch::channel(false).0,
            }),
        }
    }

    /// Begin the warm-up in the background and return immediately.
    ///
    /// Called once the gateway is listening, so the scan happens while the user
    /// is still looking at the window rather than inside their first request.
    pub fn start(&self) {
        self.begin();
    }

    /// Start the one run, if nobody has. Returns without waiting for it.
    fn begin(&self) {
        if self.inner.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let warm_up = self.clone();
        tokio::spawn(async move {
            // Supervised, so a panic in an adapter is this task's problem and
            // not a waiter's: a waiter is promised a wait, not an unwind from
            // work it did not start, and must not be parked forever either.
            let running = tokio::spawn({
                let warm_up = warm_up.clone();
                async move { warm_up.run().await }
            });
            let outcome = running.await.unwrap_or_else(|_| {
                Err(WarmUpError::Provider(ProviderFailure {
                    error: AgentError::Protocol("warm-up task did not finish".into()),
                    cleanup_unconfirmed: true,
                }))
            });
            warm_up.report(&outcome);
            warm_up.inner.settled.send_replace(true);
        });
    }

    fn report(&self, outcome: &Result<Outcome, WarmUpError>) {
        match outcome {
            Ok(Outcome::AlreadyWarm) => tracing::debug!(
                model = self.inner.runtime.model(),
                "runtime already warmed; skipping"
            ),
            Ok(Outcome::Warmed) => tracing::info!(
                model = self.inner.runtime.model(),
                "runtime warmed off the request path"
            ),
            // A failed warm-up is survivable: the next request opens its own
            // provider and reports its own outcome. It is not recorded as
            // complete, so the next start tries again.
            Err(error) => tracing::warn!(
                model = self.inner.runtime.model(),
                %error,
                "runtime warm-up did not complete"
            ),
        }
    }

    /// Wait for the single run to finish, starting it if nobody has yet.
    ///
    /// Every caller joins the same run, which is the point: a first message
    /// arriving mid-warm-up must not start a second cold launch. Returns when
    /// the run has settled, successfully or not. Cancelling this wait abandons
    /// only the wait — the run keeps going and the next caller joins it.
    pub async fn wait_until_settled(&self) {
        self.begin();
        let mut settled = self.inner.settled.subscribe();
        // The sender lives in the shared `Inner` this clone holds, so the
        // channel cannot close while anyone is still waiting on it.
        while !*settled.borrow_and_update() {
            if settled.changed().await.is_err() {
                return;
            }
        }
    }

    async fn run(&self) -> Result<Outcome, WarmUpError> {
        if self.inner.records.completed(&self.inner.runtime).await? {
            return Ok(Outcome::AlreadyWarm);
        }
        let requested_at_ms = self.inner.clock.unix_milliseconds();
        let correlation_id = Uuid::new_v4().to_string();
        let actor = ActionContext::new(GATEWAY_PRINCIPAL, WARM_UP_SURFACE, &correlation_id)
            .map_err(|error| {
                WarmUpError::Provider(ProviderFailure {
                    error: AgentError::InvalidInput(error.to_string()),
                    cleanup_unconfirmed: false,
                })
            })?;
        let (session_id, failure) = match self.open_and_close(&actor).await {
            Ok(id) => (id, None),
            Err(failure) => (None, Some(failure)),
        };
        // Evidence before the completion record: a warm-up whose audit was
        // rejected must not leave behind a record saying it succeeded.
        self.inner
            .audit
            .record(WarmUpAuditRecord {
                runtime: self.inner.runtime.clone(),
                before: WarmUpState::Cold,
                after: if failure.is_some() {
                    WarmUpState::Cold
                } else {
                    WarmUpState::Warmed
                },
                cause: WarmUpCause::AutomaticPreparation,
                initiator: actor.clone(),
                session_id: session_id.clone(),
                failure: failure.clone(),
                correlation_id,
                requested_at_ms,
                observed_at_ms: self.inner.clock.unix_milliseconds(),
            })
            .await?;
        if let Some(failure) = failure {
            return Err(WarmUpError::Provider(failure));
        }
        self.inner
            .records
            .record_completed(
                self.inner.runtime.clone(),
                self.inner.clock.unix_milliseconds(),
            )
            .await?;
        Ok(Outcome::Warmed)
    }

    /// One real session: the provider is launched, asked to establish a
    /// session, and closed again. The storage it is given is composition's
    /// choice and is thrown away — this context must not leave a snapshot
    /// behind, and must not take an exclusive lease on a conversation a user
    /// owns.
    async fn open_and_close(
        &self,
        actor: &ActionContext,
    ) -> Result<Option<String>, ProviderFailure> {
        let manager = SessionManager::open(None, self.inner.storage.clone())
            .await
            .map_err(|error| ProviderFailure {
                error: AgentError::Storage(error),
                cleanup_unconfirmed: false,
            })?;
        let agent = Agent::prepare(
            self.inner.provider.clone(),
            manager,
            self.inner.execution_audit.clone(),
        )
        .await
        .map_err(|error| ProviderFailure {
            // Preparation can retain storage or audit cleanup ownership whose
            // release the SDK has not confirmed. Provider startup begins only
            // after the caller-attributed authorization below.
            cleanup_unconfirmed: error.needs_cleanup(),
            error: error.cause().clone(),
        })?;
        let authorization = agent
            .authorize_attachment(AttachmentRequest::CallerRequested(actor.clone()))
            .map_err(provider_failure)?;
        let attachment = agent
            .start_attachment(authorization)
            .map_err(provider_failure)?;
        if let Err(error) = attachment.wait().await {
            return Err(ProviderFailure {
                cleanup_unconfirmed: matches!(
                    agent.attachment_status().phase(),
                    AttachmentPhase::Failed(AttachmentFailureCode::Cleanup)
                ),
                error,
            });
        }
        // None rather than an empty identity: the provider not naming a session
        // is not the same fact as it naming the empty one.
        let session_id = agent
            .session_manager()
            .snapshot()
            .await
            .and_then(|snapshot| {
                snapshot
                    .provider_context
                    .recorded()
                    .map(|id| id.as_str().to_owned())
            });
        // Closing is part of the warm-up, not cleanup after it: the provider's
        // own closure evidence is what records that this session existed.
        agent
            .close(actor.clone())
            .await
            .map_err(|error| ProviderFailure {
                cleanup_unconfirmed: matches!(
                    error,
                    AgentError::CleanupUncertain | AgentError::AuditAndCleanupFailure
                ),
                error,
            })?;
        Ok(session_id)
    }
}

fn provider_failure(error: AgentError) -> ProviderFailure {
    ProviderFailure {
        cleanup_unconfirmed: matches!(
            error,
            AgentError::CleanupUncertain
                | AgentError::AuditAndCleanupFailure
                | AgentError::OperationAndCleanupFailure { .. }
        ),
        error,
    }
}

enum Outcome {
    AlreadyWarm,
    Warmed,
}

#[cfg(test)]
#[path = "../../../tests/agent_warm_up/service.rs"]
mod tests;
