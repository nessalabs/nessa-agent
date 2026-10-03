use crate::domain::pairing::{AttemptId, ConsentIntentId, DeviceKey, InvitationId};
/// Possession of an Ed25519 device key proved by a complete native TLS handshake.
/// Only the concrete cryptographic adapter can construct this evidence.
#[derive(Debug)]
pub struct DeviceConnectionProof {
    key: DeviceKey,
}
impl DeviceConnectionProof {
    pub(crate) fn from_tls(key: DeviceKey) -> Self {
        Self { key }
    }
    /// Exact permanent key proved by the native channel.
    pub fn key(&self) -> DeviceKey {
        self.key
    }
}
/// Successful native OPAQUE confirmation tied to one locally derived channel context.
/// A key string or DTO cannot construct this evidence.
pub struct ConfirmedClaim {
    invitation: InvitationId,
    attempt: AttemptId,
    consent: ConsentIntentId,
    generation: u64,
    expiry_ms: u64,
    key: DeviceKey,
    input: [u8; 32],
}
impl ConfirmedClaim {
    pub(crate) fn from_pake(
        invitation: InvitationId,
        attempt: AttemptId,
        consent: ConsentIntentId,
        generation: u64,
        expiry_ms: u64,
        key: DeviceKey,
        input: [u8; 32],
    ) -> Self {
        Self {
            invitation,
            attempt,
            consent,
            generation,
            expiry_ms,
            key,
            input,
        }
    }
    /// Invitation whose selected PAKE authenticated the code.
    pub fn invitation(&self) -> InvitationId {
        self.invitation
    }
    /// Exact charged attempt authenticated by KE3.
    pub fn attempt(&self) -> AttemptId {
        self.attempt
    }
    /// Immutable exact intent correlation authenticated by the native transcript.
    pub fn consent(&self) -> ConsentIntentId {
        self.consent
    }
    /// Intent generation authenticated by the native transcript.
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// Exclusive invitation expiry authenticated by the native transcript.
    pub fn expiry_ms(&self) -> u64 {
        self.expiry_ms
    }
    /// Permanent key independently proved by TLS and bound by PAKE.
    pub fn key(&self) -> DeviceKey {
        self.key
    }
    /// Exact charged KE1 fingerprint; identifies duplicate input, conveys no authority alone.
    pub fn input(&self) -> [u8; 32] {
        self.input
    }
}
