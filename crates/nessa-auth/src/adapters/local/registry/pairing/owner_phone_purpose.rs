//! Purpose comes from actual issued DevicePairing proof, never coarse action grants.
use super::super::{DeviceProofBinding, LocalCredentialStore, StoredProof};
use crate::{
    application::{
        pairing::{OwnerPhonePurpose, OwnerPhonePurposeSource},
        ports::{AccessError, Clock, PortFuture},
        session::{AuthenticatedSession, ReadCurrentSession},
    },
    domain::pairing::ConsentClass,
};
impl OwnerPhonePurposeSource for LocalCredentialStore {
    fn owner_phone_purpose<'a>(
        &'a self,
        session: &'a AuthenticatedSession,
        clock: &'a dyn Clock,
    ) -> PortFuture<'a, Option<OwnerPhonePurpose>> {
        Box::pin(async move {
            let snapshot = ReadCurrentSession {
                access: self,
                clock,
            }
            .execute(session)
            .await?;
            self.with_published_registry(|registry| {
                if registry.revision != snapshot.revision {
                    return Err(AccessError::StaleRevision);
                }
                let credential = registry
                    .credentials
                    .iter()
                    .find(|entry| entry.metadata.id == session.context().credential_id().as_str())
                    .ok_or(AccessError::Unavailable)?;
                let StoredProof::Device(DeviceProofBinding::DevicePairing {
                    invitation,
                    generation,
                }) = &credential.verifier
                else {
                    return Ok(None);
                };
                let pairing = registry
                    .pairings
                    .iter()
                    .find(|entry| entry.id().bytes() == invitation)
                    .ok_or(AccessError::Unavailable)?;
                let record = pairing
                    .restore(&registry.transitions)
                    .map_err(|_| AccessError::Unavailable)?;
                if record.intent().generation() != *generation
                    || record.credential() != Some(session.context().credential_id())
                {
                    return Err(AccessError::IdentityMismatch);
                }
                if record.intent().class() != ConsentClass::OwnerPhoneReadWrite {
                    return Ok(None);
                }
                OwnerPhonePurpose::from_registry(&record, session.context()).map(Some)
            })
        })
    }
}
