use super::ports::{ProviderFailure, WarmUpAudit, WarmUpAuditRecord, WarmUpError, WarmUpRecords};
use crate::agent_warm_up::domain::{RuntimeFingerprint, WarmUpState};
use nessa_auth::application::ports::Clock;
use nessa_sdk::application::agent_execution::{
    agents::{Agent, AgentError},
    permissions::ActionContext,
    providers::AgentProvider,
    sessions::{SessionManager, SessionStorage},
};
use std::sync::Arc;
use tokio::sync::OnceCell;
use uuid::Uuid;

/// Surface recorded as the initiator of a warm-up. The gateway acts as its own
/// principal here; no human asked for this and none is claimed.
const WARM_UP_SURFACE: &str = "runtime_warm_up";
const GATEWAY_PRINCIPAL: &str = "gateway";

/// Runs the configured runtime through a full open and close, once, so the
/// operating system's first-execution scan is paid before anyone is waiting.
///
/// Cloning shares the single run. Whoever asks first performs it; everyone else
/// waits for the same one, which is the whole point — a first message arriving
/// mid-warm-up must not start a second cold launch.
#[derive(Clone)]
pub struct AgentWarmUp {
    inner: Arc<Inner>,
}
struct Inner {
    provider: Arc<dyn AgentProvider>,
    storage: Arc<dyn SessionStorage>,
    records: Arc<dyn WarmUpRecords>,
    audit: Arc<dyn WarmUpAudit>,
    clock: Arc<dyn Clock>,
    runtime: RuntimeFingerprint,
    settled: OnceCell<()>,
}

impl AgentWarmUp {
    /// Prepare a warm-up for one configured runtime. Nothing runs until
    /// [`Self::start`] or a waiting caller asks for it.
    pub fn new(
        provider: Arc<dyn AgentProvider>,
        storage: Arc<dyn SessionStorage>,
        records: Arc<dyn WarmUpRecords>,
        audit: Arc<dyn WarmUpAudit>,
        clock: Arc<dyn Clock>,
        runtime: RuntimeFingerprint,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                provider,
                storage,
                records,
                audit,
                clock,
                runtime,
                settled: OnceCell::new(),
            }),
        }
    }

    /// Begin the warm-up in the background and return immediately.
    ///
    /// Called once the gateway is listening, so the scan happens while the user
    /// is still looking at the window rather than inside their first request.
    pub fn start(&self) {
        let warm_up = self.clone();
        tokio::spawn(async move { warm_up.wait_until_settled().await });
    }

    /// Wait for the single run to finish, starting it if nobody has yet.
    ///
    /// Every caller joins the same run, which is the point: a first message
    /// arriving mid-warm-up must not start a second cold launch. Returns when
    /// the run has settled, successfully or not.
    pub async fn wait_until_settled(&self) {
        self.inner
            .settled
            .get_or_init(|| async {
                // Supervised, so a panic in a provider adapter is this task's
                // problem and not the caller's. A conversation waiting here is
                // promised a wait, not a failure, and must not be handed an
                // unwind from work it did not start.
                let warm_up = self.clone();
                let outcome = match tokio::spawn(async move { warm_up.run().await }).await {
                    Ok(outcome) => outcome,
                    Err(_) => Err(WarmUpError::Provider(ProviderFailure {
                        error: AgentError::Protocol("warm-up task did not finish".into()),
                        cleanup_unconfirmed: true,
                    })),
                };
                match outcome {
                    Ok(Outcome::AlreadyWarm) => tracing::debug!(
                        model = self.inner.runtime.model(),
                        "runtime already warmed; skipping"
                    ),
                    Ok(Outcome::Warmed) => tracing::info!(
                        model = self.inner.runtime.model(),
                        "runtime warmed off the request path"
                    ),
                    // A failed warm-up is survivable: the next request opens its
                    // own provider and reports its own outcome. It is not
                    // recorded as complete, so the next start tries again.
                    Err(error) => tracing::warn!(
                        model = self.inner.runtime.model(),
                        %error,
                        "runtime warm-up did not complete"
                    ),
                }
            })
            .await;
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
        let agent = Agent::new(self.inner.provider.clone(), manager)
            .await
            .map_err(|error| ProviderFailure {
                // A failed opening can retain provider resources whose release
                // the SDK has not confirmed. Recording that is the difference
                // between "the launch failed" and "the launch failed and may
                // still be running".
                cleanup_unconfirmed: error.needs_cleanup(),
                error: error.cause().clone(),
            })?;
        // None rather than an empty identity: the provider not naming a session
        // is not the same fact as it naming the empty one.
        let session_id = agent
            .session_manager()
            .snapshot()
            .await
            .map(|snapshot| snapshot.provider_session_id.as_str().to_owned());
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

enum Outcome {
    AlreadyWarm,
    Warmed,
}

#[cfg(test)]
#[path = "../../../tests/agent_warm_up/service.rs"]
mod tests;
