//! One actual gateway Argon2 registration worker, including detached callers.
use super::worker::worker_fault;
use nessa_auth::{
    adapters::pairing::{
        CryptoRng, ManualCode, NativeIdentity, PairingCryptoError, RngCore, ServerInvitation,
    },
    application::pairing::PairingWorkerFault,
    domain::pairing::InvitationId,
};
use std::sync::Arc;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

/// Registration admission and physical worker failures, without secret diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistrationError {
    /// Another physical KSF job is running, or shutdown excludes admission.
    Busy,
    /// Fixed cryptographic operation failed before publication.
    Crypto(PairingCryptoError),
    /// An unexpected worker fault occurred; owned output is not published.
    WorkerFault(PairingWorkerFault),
}
/// Secret owners returned only to the trusted gateway enrollment use case.
/// Neither owner serializes or formats secrets; code display follows registry commit.
pub struct RegisteredInvitation {
    /// Volatile password file/setup, retained until claim or terminal cleanup.
    pub setup: ServerInvitation,
    /// One-time local display; owned normalized/display bytes erase on drop.
    pub code: ManualCode,
    // Original registration remains charged through publication or refusal.
    _permit: PhysicalPermit,
}
/// A single physical KSF worker. Queuing is refused rather than retained.
/// Caller cancellation drops its waiter, while the physical closure keeps its
/// permit and secrets until completion/unwind. Composition must call shutdown
/// and await it before declaring crypto work drained.
pub struct RegistrationWorker {
    capacity: Arc<Semaphore>,
    drained: Arc<Notify>,
}
impl RegistrationWorker {
    /// Construct the accepted fixed one-worker gateway policy.
    pub fn new() -> Self {
        Self {
            capacity: Arc::new(Semaphore::new(1)),
            drained: Arc::new(Notify::new()),
        }
    }
    /// Register one random code with locally executed OPAQUE roles. Entropy is
    /// injected and moves into the same physical worker as the permit. No durable
    /// enrollment or code acknowledgement exists until the caller commits it.
    pub async fn register<R: RngCore + CryptoRng + Send + 'static>(
        &self,
        mut entropy: R,
        id: InvitationId,
        gateway: &NativeIdentity,
    ) -> Result<RegisteredInvitation, RegistrationError> {
        let gateway = gateway.public_spki();
        let permit = self
            .capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| RegistrationError::Busy)?;
        let permit = PhysicalPermit {
            permit: Some(permit),
            drained: self.drained.clone(),
        };
        tokio::task::spawn_blocking(move || {
            let code = ManualCode::generate(&mut entropy);
            let setup = ServerInvitation::register(&mut entropy, &code, id, gateway)
                .map_err(RegistrationError::Crypto)?;
            Ok(RegisteredInvitation {
                setup,
                code,
                _permit: permit,
            })
        })
        .await
        .map_err(|error| RegistrationError::WorkerFault(worker_fault(error)))?
    }
    /// Exclude new jobs, then wait until the last physical closure releases its
    /// actual permit, even when the caller was dropped
    /// (`native_create_observer_loss_keeps_original_owner_until_drain`).
    pub async fn shutdown(&self) {
        self.capacity.close();
        loop {
            let notified = self.drained.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.capacity.available_permits() == 1 {
                return;
            }
            notified.await;
        }
    }
}
impl Default for RegistrationWorker {
    fn default() -> Self {
        Self::new()
    }
}
struct PhysicalPermit {
    permit: Option<OwnedSemaphorePermit>,
    drained: Arc<Notify>,
}
impl Drop for PhysicalPermit {
    fn drop(&mut self) {
        drop(self.permit.take());
        self.drained.notify_waiters();
    }
}
