use super::{
    ports::{ProviderFailure, WarmUpAudit, WarmUpAuditRecord, WarmUpError, WarmUpRecords},
    terminal::{
        WarmUpAuditDelivery, WarmUpCompletionRecordDelivery, WarmUpEffect, WarmUpLaunchOwnership,
        WarmUpTerminal,
    },
};
use crate::agent_warm_up::domain::{RuntimeFingerprint, WarmUpCause, WarmUpState};
use nessa_auth::application::ports::Clock;
use nessa_sdk::application::agent_execution::{
    agents::{Agent, AgentError, AgentInitializationError, AttachmentRequest},
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
    settled: watch::Sender<Option<Arc<PublishedTerminal>>>,
}

struct PublishedTerminal {
    projection: Arc<WarmUpTerminal>,
    _retained: Option<RetainedWarmUpOwnership>,
    diagnostic: Option<WarmUpError>,
}

enum RetainedWarmUpOwnership {
    Initialization { _owner: AgentInitializationError },
    Attachment { _owner: Agent },
    Unknown,
}

struct PreparationAttempt {
    session_id: Option<String>,
    failure: Option<ProviderFailure>,
    retained: Option<RetainedWarmUpOwnership>,
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
                settled: watch::channel(None).0,
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
            let terminal = running.await.unwrap_or_else(|_| warm_up.lost_worker());
            warm_up.report(&terminal);
            warm_up.inner.settled.send_replace(Some(Arc::new(terminal)));
        });
    }

    fn report(&self, terminal: &PublishedTerminal) {
        match terminal.projection.effect() {
            WarmUpEffect::AlreadyPrepared => tracing::debug!(
                model = terminal.projection.runtime().model(),
                audit_delivery = ?terminal.projection.audit_delivery(),
                completion_record_delivery = ?terminal.projection.completion_record_delivery(),
                "runtime already warmed; skipping"
            ),
            WarmUpEffect::Prepared => tracing::info!(
                model = terminal.projection.runtime().model(),
                audit_delivery = ?terminal.projection.audit_delivery(),
                completion_record_delivery = ?terminal.projection.completion_record_delivery(),
                "runtime warmed off the request path"
            ),
            WarmUpEffect::Failed | WarmUpEffect::Unknown => tracing::warn!(
                model = terminal.projection.runtime().model(),
                error = terminal.diagnostic.as_ref().map(ToString::to_string),
                cleanup_retained = matches!(
                    terminal.projection.launch_ownership(),
                    WarmUpLaunchOwnership::Retained
                ),
                audit_delivery = ?terminal.projection.audit_delivery(),
                completion_record_delivery = ?terminal.projection.completion_record_delivery(),
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
        let _ = self.wait_for_terminal().await;
    }

    /// Wait for this run's immutable terminal projection.
    ///
    /// The projection keeps preparation, launch ownership, audit delivery and
    /// completion-record delivery separate. A retained launch owner is kept by
    /// this warm-up for the process lifetime; callers must not infer release
    /// from the fact that the run settled.
    pub(crate) async fn wait_for_terminal(&self) -> Arc<WarmUpTerminal> {
        self.begin();
        let mut settled = self.inner.settled.subscribe();
        // The sender lives in the shared `Inner` this clone holds, so the
        // channel cannot close while anyone is still waiting on it.
        loop {
            if let Some(terminal) = settled.borrow_and_update().as_ref() {
                return terminal.projection.clone();
            }
            if settled.changed().await.is_err() {
                unreachable!("warm-up owns its terminal sender")
            }
        }
    }

    fn terminal(
        &self,
        effect: WarmUpEffect,
        launch_ownership: WarmUpLaunchOwnership,
        audit_delivery: WarmUpAuditDelivery,
        completion_record_delivery: WarmUpCompletionRecordDelivery,
        retained: Option<RetainedWarmUpOwnership>,
        diagnostic: Option<WarmUpError>,
    ) -> PublishedTerminal {
        PublishedTerminal {
            projection: Arc::new(WarmUpTerminal::new(
                self.inner.runtime.clone(),
                effect,
                launch_ownership,
                audit_delivery,
                completion_record_delivery,
            )),
            _retained: retained,
            diagnostic,
        }
    }

    fn lost_worker(&self) -> PublishedTerminal {
        self.terminal(
            WarmUpEffect::Unknown,
            WarmUpLaunchOwnership::Retained,
            WarmUpAuditDelivery::Unknown,
            WarmUpCompletionRecordDelivery::Unknown,
            Some(RetainedWarmUpOwnership::Unknown),
            Some(WarmUpError::Provider(ProviderFailure {
                error: AgentError::Protocol("warm-up task did not finish".into()),
                cleanup_unconfirmed: true,
            })),
        )
    }

    async fn run(&self) -> PublishedTerminal {
        match self.inner.records.completed(&self.inner.runtime).await {
            Ok(true) => {
                return self.terminal(
                    WarmUpEffect::AlreadyPrepared,
                    WarmUpLaunchOwnership::Released,
                    WarmUpAuditDelivery::NotAttempted,
                    WarmUpCompletionRecordDelivery::Existing,
                    None,
                    None,
                )
            }
            Ok(false) => {}
            Err(error) => {
                return self.terminal(
                    WarmUpEffect::Failed,
                    WarmUpLaunchOwnership::Released,
                    WarmUpAuditDelivery::NotAttempted,
                    WarmUpCompletionRecordDelivery::ReadRejected,
                    None,
                    Some(error),
                )
            }
        }
        let requested_at_ms = self.inner.clock.unix_milliseconds();
        let correlation_id = Uuid::new_v4().to_string();
        let actor = match ActionContext::new(GATEWAY_PRINCIPAL, WARM_UP_SURFACE, &correlation_id) {
            Ok(actor) => actor,
            Err(error) => {
                return self.terminal(
                    WarmUpEffect::Failed,
                    WarmUpLaunchOwnership::Released,
                    WarmUpAuditDelivery::NotAttempted,
                    WarmUpCompletionRecordDelivery::NotAttempted,
                    None,
                    Some(WarmUpError::Provider(ProviderFailure {
                        error: AgentError::InvalidInput(error.to_string()),
                        cleanup_unconfirmed: false,
                    })),
                )
            }
        };
        let attempt = self.open_and_close(&actor).await;
        let ownership = if attempt.retained.is_some() {
            WarmUpLaunchOwnership::Retained
        } else {
            WarmUpLaunchOwnership::Released
        };
        let effect = if attempt.failure.is_some() {
            WarmUpEffect::Failed
        } else {
            WarmUpEffect::Prepared
        };
        let diagnostic = attempt.failure.clone().map(WarmUpError::Provider);
        let audit = self
            .inner
            .audit
            .record(WarmUpAuditRecord {
                runtime: self.inner.runtime.clone(),
                before: WarmUpState::Cold,
                after: if attempt.failure.is_some() {
                    WarmUpState::Cold
                } else {
                    WarmUpState::Warmed
                },
                cause: WarmUpCause::AutomaticPreparation,
                initiator: actor,
                session_id: attempt.session_id,
                failure: attempt.failure.clone(),
                correlation_id,
                requested_at_ms,
                observed_at_ms: self.inner.clock.unix_milliseconds(),
            })
            .await;
        if let Err(error) = audit {
            tracing::error!(
                %error,
                provider_failure = attempt.failure.as_ref().map(ToString::to_string),
                "runtime warm-up audit was rejected"
            );
            return self.terminal(
                effect,
                ownership,
                WarmUpAuditDelivery::Rejected,
                WarmUpCompletionRecordDelivery::NotAttempted,
                attempt.retained,
                diagnostic.or(Some(error)),
            );
        }
        if attempt.failure.is_some() {
            return self.terminal(
                effect,
                ownership,
                WarmUpAuditDelivery::Acknowledged,
                WarmUpCompletionRecordDelivery::NotAttempted,
                attempt.retained,
                diagnostic,
            );
        }
        match self
            .inner
            .records
            .record_completed(
                self.inner.runtime.clone(),
                self.inner.clock.unix_milliseconds(),
            )
            .await
        {
            Ok(()) => self.terminal(
                WarmUpEffect::Prepared,
                WarmUpLaunchOwnership::Released,
                WarmUpAuditDelivery::Acknowledged,
                WarmUpCompletionRecordDelivery::Recorded,
                None,
                None,
            ),
            Err(error) => self.terminal(
                WarmUpEffect::Prepared,
                WarmUpLaunchOwnership::Released,
                WarmUpAuditDelivery::Acknowledged,
                WarmUpCompletionRecordDelivery::WriteRejected,
                None,
                Some(error),
            ),
        }
    }

    /// One real session: the provider is launched, asked to establish a
    /// session, and closed again. Resource-owning failures remain owned here
    /// until the terminal projection has captured their release meaning.
    async fn open_and_close(&self, actor: &ActionContext) -> PreparationAttempt {
        let manager = match SessionManager::open(None, self.inner.storage.clone()).await {
            Ok(manager) => manager,
            Err(error) => {
                return PreparationAttempt {
                    session_id: None,
                    failure: Some(ProviderFailure {
                        error: AgentError::Storage(error),
                        cleanup_unconfirmed: false,
                    }),
                    retained: None,
                }
            }
        };
        let agent = match Agent::prepare(
            self.inner.provider.clone(),
            manager,
            self.inner.execution_audit.clone(),
        )
        .await
        {
            Ok(agent) => agent,
            Err(error) => {
                let cause = error.cause().clone();
                let retry = if error.needs_cleanup() {
                    error.retry_cleanup().await.err()
                } else {
                    None
                };
                let pending = error.needs_cleanup();
                return PreparationAttempt {
                    session_id: None,
                    failure: Some(ProviderFailure {
                        error: combine(cause, retry),
                        cleanup_unconfirmed: pending,
                    }),
                    retained: pending
                        .then_some(RetainedWarmUpOwnership::Initialization { _owner: error }),
                };
            }
        };
        let authorization =
            match agent.authorize_attachment(AttachmentRequest::CallerRequested(actor.clone())) {
                Ok(authorization) => authorization,
                Err(error) => {
                    let close = agent.close(actor.clone()).await.err();
                    return failed_agent_attempt(agent, None, error, close);
                }
            };
        let attachment = match agent.start_attachment(authorization) {
            Ok(attachment) => attachment,
            Err(error) => {
                let close = agent.close(actor.clone()).await.err();
                return failed_agent_attempt(agent, None, error, close);
            }
        };
        if let Err(error) = attachment.wait().await {
            let session_id = recorded_session_id(&agent).await;
            let close = agent.close(actor.clone()).await.err();
            return failed_agent_attempt(agent, session_id, error, close);
        }
        let session_id = recorded_session_id(&agent).await;
        if let Err(error) = agent.close(actor.clone()).await {
            let retry = if agent.attachment_cleanup_pending() {
                agent.close(actor.clone()).await.err()
            } else {
                None
            };
            return failed_agent_attempt(agent, session_id, error, retry);
        }
        PreparationAttempt {
            session_id,
            failure: None,
            retained: None,
        }
    }
}

async fn recorded_session_id(agent: &Agent) -> Option<String> {
    agent
        .session_manager()
        .snapshot()
        .await
        .and_then(|snapshot| {
            snapshot
                .provider_context
                .recorded()
                .map(|id| id.as_str().to_owned())
        })
}

fn failed_agent_attempt(
    agent: Agent,
    session_id: Option<String>,
    failure: AgentError,
    cleanup_failure: Option<AgentError>,
) -> PreparationAttempt {
    let pending = agent.attachment_cleanup_pending();
    PreparationAttempt {
        session_id,
        failure: Some(ProviderFailure {
            error: combine(failure, cleanup_failure),
            cleanup_unconfirmed: pending,
        }),
        retained: pending.then_some(RetainedWarmUpOwnership::Attachment { _owner: agent }),
    }
}

fn combine(first: AgentError, second: Option<AgentError>) -> AgentError {
    second.map_or(first.clone(), |second| {
        if first == second {
            first
        } else {
            AgentError::MultipleOperationFailures {
                first_error: Box::new(first),
                subsequent_error: Box::new(second),
            }
        }
    })
}

#[cfg(test)]
#[path = "../../../tests/agent_warm_up/service.rs"]
mod tests;
