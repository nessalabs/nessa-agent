//! Original setup receipts share the Auth registry transaction and physical stage owner.
use super::{
    DeviceConnectionProof, PairingAdmission, PairingStoreError, VerifiedOwnerPhoneDeviceProof,
};
use crate::{
    application::ports::{Clock, PortFuture},
    domain::pairing::{OwnerPhoneRequest, OwnerPhoneRequestId, OwnerPhoneSource, PairingRecord},
};

/// Signed original owner authorization; no caller can manufacture key proof from a DTO.
pub trait OwnerPhoneStore: Send + Sync {
    /// Locate original intent for subsequent fresh admission. This internal projection
    /// is not a product endpoint or authorization; callers must verify the current original
    /// session and admit the returned exact intent before exposing or settling it.
    fn find_owner_phone(
        &self,
        source: &OwnerPhoneSource,
        request: OwnerPhoneRequestId,
    ) -> Result<PairingRecord, PairingStoreError>;
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
    ) -> Result<PairingRecord, PairingStoreError>;
    /// Recover only this original request under actual TLS device possession. This read
    /// cannot stage, publish, grant, cancel, or accept an unrelated original request.
    fn recover_owner_phone(
        &self,
        request: OwnerPhoneRequestId,
        digest: &[u8; 32],
        proof: &DeviceConnectionProof,
    ) -> Result<PairingRecord, PairingStoreError>;
}
