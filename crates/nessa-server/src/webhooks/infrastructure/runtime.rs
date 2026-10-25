//! Concrete product command owner. Caller loss does not cancel admitted work.
#[path = "runtime/delivery.rs"]
mod delivery;
use super::SqlOutbox;
use crate::conversation::application::ConversationError;
use crate::webhooks::application::{
    ExpiryObserver, ObserveRegistrationStop, OutboxError, RegisterWebhook, RegistrationConfig,
    RegistrationFailure, RegistrationIntent, StopObservationResult, MAX_OUTBOX_WORKERS,
    MAX_REGISTRATIONS,
};
pub(crate) use delivery::DeliveryDependencies;
use delivery::KnownObservation;
use futures_util::FutureExt;
use nessa_auth::adapters::local::LocalCredentialStore;
use nessa_auth::application::{
    credential_admin::{
        CredentialAdmin, CredentialAdminError, IssueCredentialOutcome, IssueCredentialRequest,
        ListCredentialsRequest, RevokeCredentialOutcome, RevokeCredentialRequest,
    },
    dto::CredentialMetadataDto,
    ports::{AccessError, AccessReader, Clock, PortFuture},
};
use nessa_auth::domain::{CredentialId, IssuanceCause, PrincipalId};
use std::collections::HashMap;
use std::fmt::{Debug, Formatter, Result as FmtResult};
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};
use tokio::sync::{oneshot, Mutex as AsyncMutex, Notify, Semaphore};
use tokio::task::{JoinHandle, JoinSet};
use uuid::Uuid;

#[derive(Clone)]
pub struct ConfiguredWebhook {
    pub principal: PrincipalId,
    pub registration: RegistrationConfig,
}
impl Debug for ConfiguredWebhook {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("ConfiguredWebhook")
            .field("principal", &self.principal)
            .finish_non_exhaustive()
    }
}
struct Commands {
    accepting: bool,
    failed: bool,
    jobs: JoinSet<bool>,
    delivery: Option<JoinHandle<Result<(), CredentialAdminError>>>,
    cleanup: Option<Arc<AsyncMutex<Cleanup>>>,
}
struct Cleanup {
    job: Option<JoinHandle<Result<(), CredentialAdminError>>>,
    result: Option<Result<(), CredentialAdminError>>,
}
impl Commands {
    fn reap(&mut self) {
        while let Some(done) = self.jobs.try_join_next() {
            if !matches!(done, Ok(true)) {
                self.failed = true;
                self.accepting = false;
            }
        }
    }
}
struct Owner {
    configurations: HashMap<String, RegistrationConfig>,
    base: Arc<dyn CredentialAdmin>,
    credentials: Arc<LocalCredentialStore>,
    store: Arc<SqlOutbox>,
    clock: Arc<dyn Clock>,
    issuance: AsyncMutex<()>,
    commands: Mutex<Commands>,
    ordinary: Arc<Semaphore>,
    controls: Arc<Semaphore>,
    wake: Notify,
    known: Mutex<Option<KnownObservation>>,
    #[cfg(test)]
    delivery_cycles: std::sync::atomic::AtomicU64,
    #[cfg(test)]
    cycle_finished: Notify,
}
#[derive(Clone)]
pub struct WebhookRuntime {
    owner: Arc<Owner>,
}
impl WebhookRuntime {
    pub fn new(
        configurations: Vec<ConfiguredWebhook>,
        base: Arc<dyn CredentialAdmin>,
        credentials: Arc<LocalCredentialStore>,
        store: Arc<SqlOutbox>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, OutboxError> {
        if configurations.len() > MAX_REGISTRATIONS {
            return Err(OutboxError::Capacity);
        }
        let mut selected = HashMap::new();
        for config in configurations {
            if selected
                .insert(config.principal.as_str().to_owned(), config.registration)
                .is_some()
            {
                return Err(OutboxError::Invalid);
            }
        }
        Ok(Self {
            owner: Arc::new(Owner {
                configurations: selected,
                base,
                credentials,
                store,
                clock,
                issuance: AsyncMutex::new(()),
                commands: Mutex::new(Commands {
                    accepting: true,
                    failed: false,
                    jobs: JoinSet::new(),
                    delivery: None,
                    cleanup: None,
                }),
                ordinary: Arc::new(Semaphore::new(MAX_OUTBOX_WORKERS)),
                controls: Arc::new(Semaphore::new(MAX_OUTBOX_WORKERS)),
                wake: Notify::new(),
                known: Mutex::new(None),
                #[cfg(test)]
                delivery_cycles: std::sync::atomic::AtomicU64::new(0),
                #[cfg(test)]
                cycle_finished: Notify::new(),
            }),
        })
    }
    #[cfg(test)]
    pub(super) fn ordinary_in_flight(&self) -> usize {
        MAX_OUTBOX_WORKERS - self.owner.ordinary.available_permits()
    }
    pub(crate) fn start_delivery(
        &self,
        dependencies: DeliveryDependencies,
    ) -> Result<(), CredentialAdminError> {
        let mut commands = self
            .owner
            .commands
            .lock()
            .map_err(|_| CredentialAdminError::Unavailable)?;
        commands.reap();
        if !commands.accepting || commands.delivery.is_some() {
            return Err(CredentialAdminError::Unavailable);
        }
        let this = self.clone();
        commands.delivery = Some(tokio::spawn(async move {
            let result = AssertUnwindSafe(this.delivery_loop(dependencies))
                .catch_unwind()
                .await
                .unwrap_or(Err(CredentialAdminError::Unavailable));
            if result.is_err() {
                if let Ok(mut commands) = this.owner.commands.lock() {
                    commands.failed = true;
                    commands.accepting = false;
                }
            }
            result
        }));
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn completed_delivery_cycles(&self) -> u64 {
        self.owner
            .delivery_cycles
            .load(std::sync::atomic::Ordering::SeqCst)
    }
    #[cfg(test)]
    pub(crate) async fn after_delivery_cycle(&self, prior: u64) {
        loop {
            let changed = self.owner.cycle_finished.notified();
            if self.completed_delivery_cycles() > prior {
                return;
            }
            changed.await;
        }
    }
    pub(crate) fn wake_delivery(&self) {
        self.owner.wake.notify_one();
    }
    pub(crate) fn store(&self) -> &SqlOutbox {
        &self.owner.store
    }
    pub(crate) fn credentials(&self) -> &LocalCredentialStore {
        &self.owner.credentials
    }
    pub(crate) async fn is_webhook(
        &self,
        credential: &CredentialId,
    ) -> Result<bool, CredentialAdminError> {
        match self.owner.credentials.read(credential).await {
            Ok(snapshot) if snapshot.credential.id() == credential => {
                Ok(snapshot.origin.cause() == IssuanceCause::WebhookIssue)
            }
            Err(AccessError::InvalidCredential) => Ok(false),
            _ => Err(CredentialAdminError::Unavailable),
        }
    }
    async fn owned<T: Send + 'static>(
        &self,
        control: bool,
        work: impl Future<Output = Result<T, CredentialAdminError>> + Send + 'static,
    ) -> Result<T, CredentialAdminError> {
        let capacity = if control {
            &self.owner.controls
        } else {
            &self.owner.ordinary
        };
        let permit = capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| CredentialAdminError::Capacity)?;
        let (send, receive) = oneshot::channel();
        {
            let mut commands = self
                .owner
                .commands
                .lock()
                .map_err(|_| CredentialAdminError::Unavailable)?;
            commands.reap();
            if !commands.accepting {
                return Err(CredentialAdminError::Unavailable);
            }
            let owner = self.owner.clone();
            commands.jobs.spawn(async move {
                let _permit = permit;
                match AssertUnwindSafe(work).catch_unwind().await {
                    Ok(result) => {
                        owner.wake.notify_one();
                        let _ = send.send(result);
                        true
                    }
                    Err(_) => {
                        // Failure is visible before the response can wake a
                        // racing caller. Reaping task exit is too late.
                        if let Ok(mut commands) = owner.commands.lock() {
                            commands.failed = true;
                            commands.accepting = false;
                        }
                        let _ = send.send(Err(CredentialAdminError::Unavailable));
                        false
                    }
                }
            });
        }
        receive
            .await
            .map_err(|_| CredentialAdminError::Unavailable)?
    }
    /// Retain an admitted registered-principal grant mutation through its
    /// original native completion. This does not replace its authorization.
    pub(crate) async fn retain_grant<T: Send + 'static>(
        &self,
        control: bool,
        work: impl Future<Output = Result<T, ConversationError>> + Send + 'static,
    ) -> Result<T, ConversationError> {
        self.owned(control, async move { Ok(work.await) })
            .await
            .map_err(|_| ConversationError::Metadata)?
    }

    async fn issue_command(
        &self,
        request: IssueCredentialRequest,
    ) -> Result<IssueCredentialOutcome, CredentialAdminError> {
        let _issuance = self.owner.issuance.lock().await;
        let Some(config) = self
            .owner
            .configurations
            .get(&request.principal.id)
            .cloned()
        else {
            return self.owner.base.issue(request).await;
        };
        for original in self.owner.store.intents().await.map_err(store_error)? {
            if original.issue.principal.id == request.principal.id
                && (original.issue.issuer_principal_id != request.issuer_principal_id
                    || original.issue.request_id != request.request_id)
            {
                return Err(CredentialAdminError::Conflict);
            }
        }
        let registered = RegisterWebhook {
            credentials: self.owner.base.as_ref(),
            admission: self.owner.credentials.as_ref(),
            intents: self.owner.store.as_ref(),
            stops: self.owner.store.as_ref(),
            clock: self.owner.clock.as_ref(),
        }
        .execute(RegistrationIntent {
            id: Uuid::new_v4(),
            issue: request,
            config,
            observed_at_ms: self.owner.clock.unix_milliseconds(),
        })
        .await
        .map_err(registration_error)?;
        Ok(IssueCredentialOutcome::ExistingSecretUnavailable {
            metadata: registered.credential,
            transitions: registered.transitions,
        })
    }
    /// Reconcile original private inputs before product/source admission starts.
    pub async fn recover(&self) -> Result<(), CredentialAdminError> {
        let this = self.clone();
        self.owned(false, async move {
            let _issuance = this.owner.issuance.lock().await;
            crate::webhooks::application::Outbox::recover(
                this.owner.store.as_ref(),
                this.owner.clock.unix_milliseconds(),
            )
            .await
            .map_err(store_error)?;
            let originals = this.owner.store.intents().await.map_err(store_error)?;
            for intent in originals {
                let credential = CredentialId::new(&intent.issue.credential_id)
                    .map_err(|_| CredentialAdminError::Unavailable)?;
                if this
                    .owner
                    .store
                    .registration(credential.clone())
                    .await
                    .map_err(store_error)?
                    .is_none()
                {
                    match (RegisterWebhook {
                        credentials: this.owner.base.as_ref(),
                        admission: this.owner.credentials.as_ref(),
                        intents: this.owner.store.as_ref(),
                        stops: this.owner.store.as_ref(),
                        clock: this.owner.clock.as_ref(),
                    })
                    .execute(intent)
                    .await
                    {
                        Ok(_) | Err(RegistrationFailure::Stopped) => {}
                        Err(error) => return Err(registration_error(error)),
                    }
                }
                (ObserveRegistrationStop {
                    credentials: this.owner.credentials.as_ref(),
                    stops: this.owner.store.as_ref(),
                    clock: this.owner.clock.as_ref(),
                })
                .execute(&credential, ExpiryObserver::Restart)
                .await
                .map_err(|_| CredentialAdminError::Unavailable)?;
            }
            Ok(())
        })
        .await
    }
    /// Every observer shares one retained physical drain. Cancelling an
    /// observer leaves both the drain task and its join handle with this owner.
    pub async fn shutdown(&self) -> Result<(), CredentialAdminError> {
        let cleanup = {
            let mut commands = self
                .owner
                .commands
                .lock()
                .map_err(|_| CredentialAdminError::Unavailable)?;
            commands.reap();
            if let Some(cleanup) = &commands.cleanup {
                cleanup.clone()
            } else {
                commands.accepting = false;
                let jobs = std::mem::take(&mut commands.jobs);
                let delivery = commands.delivery.take();
                self.owner.wake.notify_one();
                let failed = commands.failed;
                let this = self.clone();
                let cleanup = Arc::new(AsyncMutex::new(Cleanup {
                    job: Some(tokio::spawn(async move {
                        this.drain_commands(jobs, delivery, failed).await
                    })),
                    result: None,
                }));
                commands.cleanup = Some(cleanup.clone());
                cleanup
            }
        };
        let mut cleanup = cleanup.lock().await;
        if let Some(result) = &cleanup.result {
            return *result;
        }
        let result = match cleanup.job.as_mut() {
            Some(job) => job.await.unwrap_or(Err(CredentialAdminError::Unavailable)),
            None => Err(CredentialAdminError::Unavailable),
        };
        cleanup.result = Some(result);
        cleanup.job = None;
        result
    }
    async fn drain_commands(
        &self,
        mut jobs: JoinSet<bool>,
        delivery: Option<JoinHandle<Result<(), CredentialAdminError>>>,
        mut failed: bool,
    ) -> Result<(), CredentialAdminError> {
        if let Some(delivery) = delivery {
            if !matches!(delivery.await, Ok(Ok(()))) {
                failed = true;
            }
        }
        if self.settle_known().await.is_err() {
            failed = true;
        }
        while let Some(done) = jobs.join_next().await {
            if !matches!(done, Ok(true)) {
                failed = true;
            }
        }
        for intent in self.owner.store.intents().await.map_err(store_error)? {
            let credential = CredentialId::new(&intent.issue.credential_id)
                .map_err(|_| CredentialAdminError::Unavailable)?;
            if self
                .owner
                .store
                .registration(credential)
                .await
                .map_err(store_error)?
                .is_none()
            {
                failed = true;
            }
        }
        if failed {
            Err(CredentialAdminError::Unavailable)
        } else {
            Ok(())
        }
    }
}
impl CredentialAdmin for WebhookRuntime {
    fn issue<'a>(
        &'a self,
        request: IssueCredentialRequest,
    ) -> PortFuture<'a, IssueCredentialOutcome, CredentialAdminError> {
        let this = self.clone();
        Box::pin(async move {
            self.owned(false, async move { this.issue_command(request).await })
                .await
        })
    }
    fn list<'a>(
        &'a self,
        request: ListCredentialsRequest,
    ) -> PortFuture<'a, Vec<CredentialMetadataDto>, CredentialAdminError> {
        self.owner.base.list(request)
    }
    fn revoke<'a>(
        &'a self,
        request: RevokeCredentialRequest,
    ) -> PortFuture<'a, RevokeCredentialOutcome, CredentialAdminError> {
        let this = self.clone();
        Box::pin(async move {
            self.owned(true, async move {
                let credential = CredentialId::new(&request.credential_id)
                    .map_err(|_| CredentialAdminError::Conflict)?;
                let outcome = this.owner.base.revoke(request).await?;
                let stopped = (ObserveRegistrationStop {
                    credentials: this.owner.credentials.as_ref(),
                    stops: this.owner.store.as_ref(),
                    clock: this.owner.clock.as_ref(),
                })
                .execute(&credential, ExpiryObserver::Restart)
                .await
                .map_err(|_| CredentialAdminError::Unavailable)?;
                if stopped != StopObservationResult::StopAcknowledged {
                    return Err(CredentialAdminError::Unavailable);
                }
                Ok(outcome)
            })
            .await
        })
    }
}
fn store_error(error: OutboxError) -> CredentialAdminError {
    match error {
        OutboxError::Conflict => CredentialAdminError::Conflict,
        OutboxError::Capacity => CredentialAdminError::Capacity,
        _ => CredentialAdminError::Unavailable,
    }
}
fn registration_error(error: RegistrationFailure) -> CredentialAdminError {
    match error {
        RegistrationFailure::Auth(error) => error,
        RegistrationFailure::Store(error) => store_error(error),
        RegistrationFailure::OriginRecoveryRequired => CredentialAdminError::OriginRecoveryRequired,
        _ => CredentialAdminError::Unavailable,
    }
}

#[cfg(test)]
#[path = "../../../tests/webhooks/infrastructure/runtime.rs"]
mod tests;
