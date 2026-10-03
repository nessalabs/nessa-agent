//! Exact-attempt status consumes the original store and live TLS proof.
use super::DevicePairingStatus;
use nessa_auth::{
    application::pairing::{DeviceConnectionProof, PairingStore, PairingStoreError},
    domain::pairing::{AttemptId, AttemptOutcome, InvitationId},
};
/// Status reads through the canonical store and actual live TLS possession proof.
pub struct ReadDevicePairing<'a> {
    /// Existing invitation/attempt authority; no volatile reconstruction.
    pub enrollments: &'a dyn PairingStore,
}
impl ReadDevicePairing<'_> {
    /// Match the exact admission and permanent key before disclosing its receipt.
    /// A key string, invitation locator or another device attempt is insufficient.
    pub fn execute(
        &self,
        id: InvitationId,
        attempt: AttemptId,
        device: &DeviceConnectionProof,
    ) -> Result<DevicePairingStatus, PairingStoreError> {
        let record = self.enrollments.read_pairing(id)?;
        let outcome = record
            .attempt_status(attempt, device.key())
            .map_err(PairingStoreError::Domain)?;
        if outcome == AttemptOutcome::Pending {
            return Ok(DevicePairingStatus::Pending);
        }
        if outcome == AttemptOutcome::Claimed {
            return Ok(DevicePairingStatus::Claimed(Box::new(record)));
        }
        Ok(DevicePairingStatus::Unclaimed {
            outcome,
            terminal: record.terminal().map(|(cause, _)| cause),
        })
    }
}
