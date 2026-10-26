//! Explicit same-owner adoption after actual pinned recovery.
use super::super::{encode_credential, encode_pending, signed, ClientRecord, ENROLLMENT_FILE};
use super::{encode, FilePairingState, ORIGINAL_FILE};
use crate::{
    adapters::pairing::{verify_owner_phone_request, NativeIdentity},
    application::pairing::{
        DeviceCredential, OwnerPhoneAdoption, OwnerPhoneAdoptionFailure, OwnerPhoneEnrollmentKind,
        OwnerPhoneEnrollmentStore, PendingEnrollment, PrivateKeyMaterial, PrivateStateError,
    },
    domain::{
        pairing::{DisclosedConsent, OwnerPhoneRequest},
        CredentialId, ResourceId,
    },
};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

impl OwnerPhoneEnrollmentStore for FilePairingState {
    fn adopt_owner_phone(
        &self,
        expected: &OwnerPhoneRequest,
        digest: &[u8; 32],
        consent: &DisclosedConsent,
        issued: Option<(&CredentialId, &ResourceId)>,
    ) -> Result<OwnerPhoneAdoption, OwnerPhoneAdoptionFailure> {
        use OwnerPhoneAdoptionFailure::{BindLineage, OriginalRead, PublishEnrollment};
        let _operation = self
            .operation
            .lock()
            .map_err(|_| OriginalRead(PrivateStateError::Unavailable))?;
        let original = self
            .load_original_record()
            .map_err(OriginalRead)?
            .ok_or(OriginalRead(PrivateStateError::Conflict))?;
        let key = original
            .key()
            .ok_or(OriginalRead(PrivateStateError::Conflict))?;
        let identity =
            NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new(*key.expose_bytes())))
                .map_err(|_| OriginalRead(PrivateStateError::Corrupt))?;
        let request = original
            .reservation()
            .context()
            .request(original.reservation().id(), identity.device_key())
            .map_err(|_| OriginalRead(PrivateStateError::Corrupt))?;
        if &request != expected {
            return Err(OriginalRead(PrivateStateError::Conflict));
        }
        let signature = identity
            .sign_owner_phone_request(&request)
            .map_err(|_| OriginalRead(PrivateStateError::Corrupt))?;
        let verified = verify_owner_phone_request(request, &signature)
            .map_err(|_| OriginalRead(PrivateStateError::Corrupt))?;
        if verified.digest() != digest {
            return Err(OriginalRead(PrivateStateError::Conflict));
        }
        expected
            .correlate_disclosure(consent)
            .map_err(|_| OriginalRead(PrivateStateError::Conflict))?;
        let public = consent.public();
        if original
            .public_intent()
            .is_some_and(|saved| saved != public)
        {
            return Err(OriginalRead(PrivateStateError::Conflict));
        }
        // The shared pure checker validates before this invocation's first ack.
        let enrollment = self.read_client_record().map_err(OriginalRead)?;
        if let Some(saved) = &enrollment {
            if !saved.signed {
                return Err(OriginalRead(PrivateStateError::Conflict));
            }
            Self::checked_owner_phone_enrollment(&original, &saved.record).map_err(OriginalRead)?;
            if let ClientRecord::Credential(saved) = &saved.record {
                let Some((credential, receiver)) = issued else {
                    return Err(OriginalRead(PrivateStateError::Conflict));
                };
                if saved.credential() != credential || saved.receiver() != receiver {
                    return Err(OriginalRead(PrivateStateError::Conflict));
                }
            }
        }
        let original_bytes =
            encode(original.reservation(), Some(key), Some(public)).map_err(BindLineage)?;
        if original.public_intent().is_some() {
            self.reconcile(ORIGINAL_FILE, &original_bytes)
                .map_err(BindLineage)?;
        } else {
            self.publish(ORIGINAL_FILE, &original_bytes, true)
                .map_err(BindLineage)?;
        }
        let bytes = signed(match issued {
            None => encode_pending(key, expected.gateway_pin(), public),
            Some((credential, receiver)) => encode_credential(&DeviceCredential::new(
                PendingEnrollment::new(
                    PrivateKeyMaterial::new(Zeroizing::new(*key.expose_bytes())),
                    *expected.gateway_pin(),
                    public,
                ),
                credential.clone(),
                receiver.clone(),
            )),
        });
        let result = match enrollment {
            Some(saved) if bool::from(saved.bytes.as_slice().ct_eq(bytes.as_slice())) => {
                self.reconcile(ENROLLMENT_FILE, &bytes)
            }
            Some(_) => self.publish(ENROLLMENT_FILE, &bytes, true),
            None => self.publish(ENROLLMENT_FILE, &bytes, false),
        };
        result.map_err(|error| PublishEnrollment {
            error,
            acknowledged_lineage: public,
        })?;
        Ok(OwnerPhoneAdoption::new(
            public,
            if issued.is_some() {
                OwnerPhoneEnrollmentKind::Issued
            } else {
                OwnerPhoneEnrollmentKind::Pending
            },
        ))
    }
}

#[cfg(test)]
#[path = "../../../../../tests/adapters/private_state/owner_phone_adoption.rs"]
mod tests;
