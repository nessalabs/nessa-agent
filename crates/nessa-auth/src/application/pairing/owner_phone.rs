//! Original setup receipts share the Auth registry transaction and physical stage owner.
use super::{
    DeviceConnectionProof, PairingAdmission, PairingStoreError, VerifiedOwnerPhoneDeviceProof,
};
use crate::{
    application::ports::{Clock, PortFuture},
    domain::pairing::{OwnerPhoneRequest, OwnerPhoneRequestId, OwnerPhoneSource, PairingRecord},
};

/// Historical signed input and its restored enrollment from one registry entry.
/// This receipt publishes facts; it is neither current admission nor a stage lease.
pub struct OwnerPhoneOriginalReceipt {
    original: OwnerPhoneRequest,
    digest: [u8; 32],
    record: PairingRecord,
}
impl OwnerPhoneOriginalReceipt {
    pub(crate) fn restored(
        original: OwnerPhoneRequest,
        digest: [u8; 32],
        record: PairingRecord,
    ) -> Self {
        Self {
            original,
            digest,
            record,
        }
    }
    /// Exact original signed source, endpoint, pin and permanent device key.
    pub fn original(&self) -> &OwnerPhoneRequest {
        &self.original
    }
    /// Digest verified by the stored original and approval restoration owner.
    pub fn digest(&self) -> &[u8; 32] {
        &self.digest
    }
    /// Enrollment restored with the original from the same locked registry revision.
    pub fn record(&self) -> &PairingRecord {
        &self.record
    }
}

/// Signed original owner authorization; no caller can manufacture key proof from a DTO.
pub trait OwnerPhoneStore: Send + Sync {
    /// Locate original intent for subsequent fresh admission. This internal projection
    /// is not a product endpoint or authorization; callers must verify the current original
    /// session and admit the returned exact intent before exposing or settling it.
    fn find_owner_phone(
        &self,
        source: &OwnerPhoneSource,
        request: OwnerPhoneRequestId,
    ) -> Result<PairingRecord, PairingStoreError> {
        self.find_owner_phone_original(source, request)
            .map(|receipt| receipt.record().clone())
    }
    /// Internal same-entry original lookup. This grants no disclosure permission;
    /// a product caller must subsequently obtain fresh original admission.
    fn find_owner_phone_original(
        &self,
        source: &OwnerPhoneSource,
        request: OwnerPhoneRequestId,
    ) -> Result<OwnerPhoneOriginalReceipt, PairingStoreError>;
    /// Atomically commit the original request and exact owner-approved record, or return
    /// its original receipt after fresh admission. Conflicting input never gets a new invitation.
    fn authorize_owner_phone<'a>(
        &'a self,
        initial: &'a PairingRecord,
        proof: &'a VerifiedOwnerPhoneDeviceProof,
        admission: &'a PairingAdmission,
        clock: &'a dyn Clock,
    ) -> PortFuture<'a, PairingRecord, PairingStoreError>;
    /// Read only the exact original request under its current original bootstrap admission.
    fn read_owner_phone(
        &self,
        request: &OwnerPhoneRequest,
        admission: &PairingAdmission,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError> {
        self.read_owner_phone_original(request, admission, clock)
            .map(|receipt| receipt.record().clone())
    }
    /// Same-entry original and record after fresh original bootstrap admission
    /// has been checked under the registry lock.
    fn read_owner_phone_original(
        &self,
        request: &OwnerPhoneRequest,
        admission: &PairingAdmission,
        clock: &dyn Clock,
    ) -> Result<OwnerPhoneOriginalReceipt, PairingStoreError>;
    /// Recover only this original request under actual TLS device possession. This read
    /// cannot stage, publish, grant, cancel, or accept an unrelated original request.
    fn recover_owner_phone(
        &self,
        request: OwnerPhoneRequestId,
        digest: &[u8; 32],
        proof: &DeviceConnectionProof,
    ) -> Result<PairingRecord, PairingStoreError> {
        self.recover_owner_phone_original(request, digest, proof)
            .map(|receipt| receipt.record().clone())
    }
    /// Read-only original recovery under actual TLS possession, exact original
    /// request identity and digest. This performs no stage or publication effect.
    fn recover_owner_phone_original(
        &self,
        request: OwnerPhoneRequestId,
        digest: &[u8; 32],
        proof: &DeviceConnectionProof,
    ) -> Result<OwnerPhoneOriginalReceipt, PairingStoreError>;
}
