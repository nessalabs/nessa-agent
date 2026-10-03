//! The owner side of one gateway's enrollment runtime, as the product socket
//! sees it: create, list, read and decide, with entropy chosen by composition.
use super::{CreatedInvitation, GatewayPairing, PairingRuntimeError};
use crate::device_pairing::application::Approval;
use nessa_auth::{
    adapters::pairing::{rand, CryptoRng, RngCore},
    application::{pairing::OwnerDecision, session::AuthenticatedSession},
    domain::pairing::{DeviceKey, InvitationId, PairingRecord},
};
use std::sync::Arc;

/// Entropy for one new invitation's identities and code.
pub trait InvitationEntropy: RngCore + CryptoRng + Send + 'static {}
impl<T: RngCore + CryptoRng + Send + 'static> InvitationEntropy for T {}

/// Where each create's entropy comes from; composition supplies the operating
/// system's generator.
pub type InvitationEntropySource = Arc<dyn Fn() -> Box<dyn InvitationEntropy> + Send + Sync>;

/// Owner commands over one gateway's enrollment runtime.
///
/// It holds the runtime privately and gives no way back to it, so whoever is
/// handed these commands cannot build a second native connection owner for the
/// same gateway (design row S13). Every command is authorized by Auth against
/// the caller's current session inside the runtime.
pub struct PairingOwnerCommands {
    gateway: Arc<GatewayPairing>,
    entropy: InvitationEntropySource,
}
impl PairingOwnerCommands {
    /// Owner commands for `gateway`, creating invitations from `entropy`.
    pub fn new(gateway: Arc<GatewayPairing>, entropy: InvitationEntropySource) -> Self {
        Self { gateway, entropy }
    }
    /// Create one invitation; the code is returned only after the registry
    /// commit (`GatewayPairing::create`).
    pub async fn create(
        &self,
        session: &AuthenticatedSession,
    ) -> Result<CreatedInvitation, PairingRuntimeError> {
        self.gateway
            .create(session.clone(), Entropy((self.entropy)()))
            .await
    }
    /// The caller's unfinished enrollments (`GatewayPairing::pending`).
    pub async fn pending(
        &self,
        session: &AuthenticatedSession,
    ) -> Result<Box<[PairingRecord]>, PairingRuntimeError> {
        self.gateway.pending(session).await
    }
    /// One enrollment, read for the caller (`GatewayPairing::owner_status`).
    pub async fn status(
        &self,
        session: &AuthenticatedSession,
        id: InvitationId,
    ) -> Result<PairingRecord, PairingRuntimeError> {
        self.gateway.owner_status(session, id).await
    }
    /// Approve the exact claimed key and carry it through to an issued
    /// credential (`GatewayPairing::approve`).
    pub async fn approve(
        &self,
        session: &AuthenticatedSession,
        id: InvitationId,
        key: DeviceKey,
    ) -> Result<Approval, PairingRuntimeError> {
        self.gateway
            .approve(session, id, key, Entropy((self.entropy)()))
            .await
    }
    /// Deny or cancel (`GatewayPairing::decide`).
    pub async fn decide(
        &self,
        session: &AuthenticatedSession,
        id: InvitationId,
        decision: OwnerDecision,
    ) -> Result<PairingRecord, PairingRuntimeError> {
        self.gateway.decide(session, id, decision).await
    }
}

/// One create's entropy, handed to the runtime as a concrete generator.
struct Entropy(Box<dyn InvitationEntropy>);
impl RngCore for Entropy {
    fn next_u32(&mut self) -> u32 {
        self.0.next_u32()
    }
    fn next_u64(&mut self) -> u64 {
        self.0.next_u64()
    }
    fn fill_bytes(&mut self, bytes: &mut [u8]) {
        self.0.fill_bytes(bytes)
    }
    fn try_fill_bytes(&mut self, bytes: &mut [u8]) -> Result<(), rand::Error> {
        self.0.try_fill_bytes(bytes)
    }
}
impl CryptoRng for Entropy {}
