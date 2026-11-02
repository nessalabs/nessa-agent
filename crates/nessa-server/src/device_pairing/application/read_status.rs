//! Exact-attempt status consumes the original store and live TLS proof.
use super::receivers::{PairingReceivers, ReceiverError, ReceiverRequest};
use nessa_auth::{
    application::{
        pairing::{DeviceConnectionProof, OwnerPhoneStore, PairingStore, PairingStoreError},
        ports::Clock,
    },
    domain::pairing::{
        AttemptId, AttemptOutcome, InvitationId, OwnerPhoneRequestId, PairingPhase, PairingRecord,
        PublicIntent,
    },
};
use nessa_protocol::pairing::DevicePairingStatus;

/// Why a device's status could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceStatusError {
    /// The registry refused or failed, including a key that is not this
    /// attempt's (`WrongActor`).
    Enrollment(PairingStoreError),
    /// An Active record's receiver could not be read, or no longer holds its
    /// pairing (`NotPaired`): no epoch is delivered then.
    Receiver(ReceiverError),
}

/// Status reads through the canonical store and actual live TLS possession proof.
pub struct ReadDevicePairing<'a> {
    /// Existing invitation/attempt authority; no volatile reconstruction.
    pub enrollments: &'a dyn PairingStore,
    /// The receiver authority, asked for an Active record's current epoch.
    pub receivers: &'a dyn PairingReceivers,
    /// Wall clock for Auth's expiry decision.
    pub clock: &'a dyn Clock,
}
impl ReadDevicePairing<'_> {
    /// Match the exact admission and permanent key before disclosing its receipt.
    /// A key string, invitation locator or another device attempt is insufficient.
    /// Only after the key matches does it ask Auth to expire a past-due record,
    /// so a refused device changes nothing. An Active record is reported with
    /// the credential it issued every time it is asked, to this key only
    /// (design row P25): a reply the device lost is delivered again, and
    /// nothing is issued anew.
    pub fn execute(
        &self,
        id: InvitationId,
        attempt: AttemptId,
        device: &DeviceConnectionProof,
    ) -> Result<DevicePairingStatus, DeviceStatusError> {
        let enrollment = DeviceStatusError::Enrollment;
        self.enrollments
            .read_pairing(id)
            .map_err(enrollment)?
            .attempt_status(attempt, device.key())
            .map_err(|error| enrollment(PairingStoreError::Domain(error)))?;
        let record = self
            .enrollments
            .expire_pairing_if_due(id, self.clock)
            .map_err(enrollment)?;
        let outcome = record
            .attempt_status(attempt, device.key())
            .map_err(|error| enrollment(PairingStoreError::Domain(error)))?;
        if outcome == AttemptOutcome::Pending {
            return Ok(DevicePairingStatus::Pending);
        }
        if outcome != AttemptOutcome::Claimed {
            return Ok(DevicePairingStatus::Unclaimed {
                outcome,
                terminal: record.terminal().map(|(cause, _)| cause),
            });
        }
        claimed_status(record, self.receivers)
    }
}

/// Original owner-phone recovery is a read under actual TLS possession, not an
/// enrollment attempt or a new owner's admission. It never expires or settles.
pub struct ReadOwnerPhoneRecovery<'a> {
    pub originals: &'a dyn OwnerPhoneStore,
    pub receivers: &'a dyn PairingReceivers,
}
impl ReadOwnerPhoneRecovery<'_> {
    pub fn execute(
        &self,
        request: OwnerPhoneRequestId,
        digest: &[u8; 32],
        device: &DeviceConnectionProof,
    ) -> Result<(PublicIntent, DevicePairingStatus), DeviceStatusError> {
        let original = self
            .originals
            .recover_owner_phone_original(request, digest, device)
            .map_err(DeviceStatusError::Enrollment)?;
        let record = original.record().clone();
        // Canonical original restoration owns the exact signed key and claim.
        // A contradictory record is refused rather than given legacy status.
        let expected = (request, device.key(), *digest);
        if record.owner_phone_approval() != Some(expected) {
            return Err(DeviceStatusError::Receiver(ReceiverError::Conflict));
        }
        // Signed owner approval has canonical public claim lineage, but no PAKE attempt.
        let attempt = AttemptId::new(*request.bytes());
        if record.claim_binding() != Some((attempt, device.key())) {
            return Err(DeviceStatusError::Receiver(ReceiverError::Conflict));
        }
        let public = PublicIntent::from_record(&record, attempt)
            .map_err(|error| DeviceStatusError::Enrollment(PairingStoreError::Domain(error)))?;
        claimed_status(record, self.receivers).map(|status| (public, status))
    }
}

/// Both recovery protocols use one factual claimed-record/current-holding projection.
pub(crate) fn claimed_status(
    record: PairingRecord,
    receivers: &dyn PairingReceivers,
) -> Result<DevicePairingStatus, DeviceStatusError> {
    if record.phase() != PairingPhase::Active {
        return Ok(DevicePairingStatus::Claimed(Box::new(record)));
    }
    // Auth publishes Active only with a stage and its receiver.
    let conflict = DeviceStatusError::Receiver(ReceiverError::Conflict);
    let request = ReceiverRequest::for_stage(&record).ok_or(conflict)?;
    let (receiver, paired_epoch) = record.receiver_binding().ok_or(conflict)?;
    let access_epoch = receivers
        .holding(&request, receiver, paired_epoch)
        .map_err(DeviceStatusError::Receiver)?
        .ok_or(DeviceStatusError::Receiver(ReceiverError::NotPaired))?;
    Ok(DevicePairingStatus::Active {
        access_epoch,
        record: Box::new(record),
        // Replaced before the status is encoded. A zero secret is not issued.
        tunnel_secret: nessa_protocol::tunnel::TunnelSecret::new([0; 32]),
    })
}
