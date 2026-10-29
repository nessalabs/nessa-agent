//! One registry commit owns both the signed original receipt and owner approval.
use super::{
    super::LocalCredentialStore, enrollment::validate_pairing_admission, projection::StoredPairing,
};
use crate::{
    application::{
        pairing::{
            DeviceConnectionProof, OwnerPhoneOriginalReceipt, OwnerPhoneStore, PairingAdmission,
            PairingStoreError, VerifiedOwnerPhoneDeviceProof,
        },
        ports::{Clock, PortFuture},
    },
    domain::pairing::{
        validate_pairing_collection, ConsentClass, OwnerPhoneRequest, OwnerPhoneRequestId,
        OwnerPhoneSource, PairingError, PairingEvent, PairingInitiator, PairingPhase,
        PairingRecord,
    },
};

fn binding(
    request: &OwnerPhoneRequest,
    admission: &PairingAdmission,
) -> Result<(), PairingStoreError> {
    let intent = admission.intent();
    let source = request.source();
    if intent.class() != ConsentClass::OwnerPhoneReadWrite
        || intent.owner() != source.owner()
        || intent.membership() != source.membership()
        || intent.audience() != source.audience()
        || intent.resource() != source.resource()
        || admission.credential() != source.credential()
    {
        return Err(PairingStoreError::Domain(PairingError::Conflict));
    }
    Ok(())
}
fn original(
    stored: &[StoredPairing],
    source: &OwnerPhoneSource,
    id: OwnerPhoneRequestId,
) -> Result<Option<usize>, PairingStoreError> {
    let mut found = None;
    for (index, entry) in stored.iter().enumerate() {
        if let Some(request) = entry
            .owner_phone_request()
            .map_err(PairingStoreError::Domain)?
        {
            if request.source() == source && request.id() == id {
                if found.is_some() {
                    return Err(PairingStoreError::Domain(PairingError::Conflict));
                }
                found = Some(index);
            }
        }
    }
    Ok(found)
}
impl OwnerPhoneStore for LocalCredentialStore {
    fn authorize_owner_phone<'a>(
        &'a self,
        initial: &'a PairingRecord,
        proof: &'a VerifiedOwnerPhoneDeviceProof,
        admission: &'a PairingAdmission,
        clock: &'a dyn Clock,
    ) -> PortFuture<'a, PairingRecord, PairingStoreError> {
        Box::pin(async move {
            let mut slot = self
                .mutation_registry()
                .map_err(|_| PairingStoreError::Unavailable)?;
            let current = slot.as_ref().ok_or(PairingStoreError::Unavailable)?;
            let now = clock.unix_milliseconds();
            validate_pairing_admission(current, admission, now, true)?;
            binding(proof.request(), admission)?;
            if let Some(index) = original(
                &current.pairings,
                proof.request().source(),
                proof.request().id(),
            )? {
                let entry = &current.pairings[index];
                if entry
                    .owner_phone_request()
                    .map_err(PairingStoreError::Domain)?
                    .as_ref()
                    != Some(proof.request())
                {
                    return Err(PairingStoreError::Domain(PairingError::Conflict));
                }
                let record = entry
                    .restore(&current.transitions)
                    .map_err(PairingStoreError::Domain)?;
                // A freshly minted intent must never mutate or authorize replay of the original.
                if record.intent() != admission.intent() {
                    return Err(PairingStoreError::Domain(PairingError::Conflict));
                }
                return Ok(record);
            }
            if initial.intent() != admission.intent()
                || initial.phase() != PairingPhase::Available
                || initial.created_at_ms() > now
            {
                return Err(PairingStoreError::Domain(PairingError::Invalid));
            }
            self.config
                .validate_pairing_count(
                    current
                        .pairings
                        .len()
                        .checked_add(1)
                        .ok_or(PairingStoreError::Domain(PairingError::Capacity))?,
                )
                .map_err(PairingStoreError::Domain)?;
            let approval = initial
                .transition(
                    PairingEvent::owner_phone_approved(
                        proof.request().id(),
                        proof.request().device_key(),
                        *proof.digest(),
                    ),
                    PairingInitiator::Principal(admission.intent().owner().clone()),
                    now,
                )
                .map_err(PairingStoreError::Domain)?;
            let record = approval.after().clone();
            let mut entry = StoredPairing::new_owner_phone(initial, proof)
                .map_err(PairingStoreError::Domain)?;
            entry
                .append(&approval, &current.transitions)
                .map_err(PairingStoreError::Domain)?;
            let mut records = current
                .pairings
                .iter()
                .map(|entry| entry.restore(&current.transitions))
                .collect::<Result<Vec<_>, _>>()
                .map_err(PairingStoreError::Domain)?;
            records.push(record.clone());
            validate_pairing_collection(&records).map_err(PairingStoreError::Domain)?;
            let mut next = current.clone();
            next.pairings.push(entry);
            next.revision = next
                .revision
                .checked_add(1)
                .ok_or(PairingStoreError::Domain(PairingError::Capacity))?;
            self.persist(&next)
                .map_err(|_| PairingStoreError::Unavailable)?;
            let revision = next.revision;
            *slot = Some(next.clone());
            self.publish_snapshot(next)
                .map_err(|_| PairingStoreError::Unavailable)?;
            self.publish_revision(revision);
            Ok(record)
        })
    }
    fn find_owner_phone_original(
        &self,
        source: &OwnerPhoneSource,
        id: OwnerPhoneRequestId,
    ) -> Result<OwnerPhoneOriginalReceipt, PairingStoreError> {
        let slot = self
            .registry()
            .map_err(|_| PairingStoreError::Unavailable)?;
        let current = slot.as_ref().ok_or(PairingStoreError::Unavailable)?;
        let index = original(&current.pairings, source, id)?.ok_or(PairingStoreError::NotFound)?;
        current.pairings[index]
            .owner_phone_receipt(&current.transitions)
            .map_err(PairingStoreError::Domain)
    }
    fn read_owner_phone_original(
        &self,
        request: &OwnerPhoneRequest,
        admission: &PairingAdmission,
        clock: &dyn Clock,
    ) -> Result<OwnerPhoneOriginalReceipt, PairingStoreError> {
        let slot = self
            .registry()
            .map_err(|_| PairingStoreError::Unavailable)?;
        let current = slot.as_ref().ok_or(PairingStoreError::Unavailable)?;
        validate_pairing_admission(current, admission, clock.unix_milliseconds(), true)?;
        // Existing-original issuer belongs to validate_pairing_admission. The
        // supplied request must equal its stored original, whose restoration
        // binds source to intent; the receipt's intent is compared below.
        let index = original(&current.pairings, request.source(), request.id())?
            .ok_or(PairingStoreError::NotFound)?;
        let entry = &current.pairings[index];
        if entry
            .owner_phone_request()
            .map_err(PairingStoreError::Domain)?
            .as_ref()
            != Some(request)
        {
            return Err(PairingStoreError::Domain(PairingError::Conflict));
        }
        let receipt = entry
            .owner_phone_receipt(&current.transitions)
            .map_err(PairingStoreError::Domain)?;
        if receipt.record().intent() != admission.intent() {
            return Err(PairingStoreError::Domain(PairingError::Conflict));
        }
        Ok(receipt)
    }
    fn recover_owner_phone_original(
        &self,
        id: OwnerPhoneRequestId,
        fingerprint: &[u8; 32],
        proof: &DeviceConnectionProof,
    ) -> Result<OwnerPhoneOriginalReceipt, PairingStoreError> {
        let slot = self
            .registry()
            .map_err(|_| PairingStoreError::Unavailable)?;
        let current = slot.as_ref().ok_or(PairingStoreError::Unavailable)?;
        let mut found = None;
        for entry in &current.pairings {
            if let Some(request) = entry
                .owner_phone_request()
                .map_err(PairingStoreError::Domain)?
            {
                if request.id() == id && request.device_key() == proof.key() {
                    let receipt = entry
                        .owner_phone_receipt(&current.transitions)
                        .map_err(PairingStoreError::Domain)?;
                    if receipt.digest() == fingerprint {
                        if found.is_some() {
                            return Err(PairingStoreError::Domain(PairingError::Conflict));
                        }
                        found = Some(receipt);
                    }
                }
            }
        }
        found.ok_or(PairingStoreError::NotFound)
    }
}
