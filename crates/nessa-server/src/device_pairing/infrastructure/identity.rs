//! Restore audited native identity before opening enrollment or listener admission.
use super::worker::worker_fault;
use nessa_auth::{
    adapters::{
        local::LocalCredentialStore,
        pairing::{CryptoRng, NativeIdentity, PairingCryptoError, RngCore},
    },
    application::{
        pairing::{
            GatewayKeyStore, PairingStore, PairingStoreError, PairingWorkerFault, PrivateStateError,
        },
        ports::Clock,
    },
    domain::AudienceId,
};
use std::sync::Arc;

/// Startup preserves private publication/audit facts and unexpected worker faults.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GatewayIdentityError {
    /// Canonical registry or all-history first-publication admission refused.
    Registry(PairingStoreError),
    /// Private key/audit state refused, with physical publication meaning retained.
    PrivateState(PrivateStateError),
    /// Native seed generation or restoration refused.
    Crypto(PairingCryptoError),
    /// The actual owned startup closure terminated unexpectedly.
    WorkerFault(PairingWorkerFault),
}
/// Trusted composition startup; returns only an acknowledged, audited native key.
/// The closure owns registry/private-store locks through physical drain even if
/// its waiter is dropped.
pub async fn restore_gateway_identity<R: RngCore + CryptoRng + Send + 'static>(
    registry: Arc<LocalCredentialStore>,
    keys: Arc<dyn GatewayKeyStore>,
    clock: Arc<dyn Clock>,
    mut entropy: R,
) -> Result<NativeIdentity, GatewayIdentityError> {
    tokio::task::spawn_blocking(move || {
        let gateway = AudienceId::new(
            registry
                .gateway_id()
                .map_err(|_| GatewayIdentityError::Registry(PairingStoreError::Unavailable))?,
        )
        .map_err(|_| GatewayIdentityError::Registry(PairingStoreError::Unavailable))?;
        if let Some(key) = keys
            .restore_gateway_key(&gateway, clock.as_ref())
            .map_err(GatewayIdentityError::PrivateState)?
        {
            return NativeIdentity::restore(key).map_err(GatewayIdentityError::Crypto);
        }
        let identity =
            NativeIdentity::generate(&mut entropy).map_err(GatewayIdentityError::Crypto)?;
        registry
            .publish_first_gateway_key(keys.as_ref(), identity.key_material(), clock.as_ref())
            .map_err(GatewayIdentityError::Registry)?;
        Ok(identity)
    })
    .await
    .map_err(|error| GatewayIdentityError::WorkerFault(worker_fault(error)))?
}
