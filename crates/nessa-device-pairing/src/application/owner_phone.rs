//! Signed first-party consent reuses the original registry intent on every replay.
use super::{Approval, FreshStage, OwnerError, PairingOwner};
use nessa_auth::{
    application::{
        pairing::{
            OwnerPhoneOriginalReceipt, OwnerPhoneStore, PairingStoreError,
            VerifiedOwnerPhoneDeviceProof,
        },
        session::AuthenticatedSession,
    },
    domain::pairing::{
        ConsentClass, ConsentIntent, ConsentIntentId, InvitationId, OwnerPhoneRequestId,
        OwnerPhoneSource, PairingError, PairingPhase, PairingRecord,
    },
};
use std::net::SocketAddr;

/// Caller-generated identities are candidates only; the first registry commit owns them.
pub struct FreshOwnerPhone {
    /// Candidate invitation identity, used only when no original exists.
    pub invitation: InvitationId,
    /// Candidate canonical consent identity for that same first commit.
    pub consent: ConsentIntentId,
}

impl PairingOwner<'_> {
    /// The signed approval and stage settlement share the same owned physical command.
    pub async fn authorize_and_activate_phone(
        &self,
        session: &AuthenticatedSession,
        originals: &dyn OwnerPhoneStore,
        proof: &VerifiedOwnerPhoneDeviceProof,
        mut mint: impl FnMut() -> Result<(FreshOwnerPhone, FreshStage), OwnerError>,
        endpoint: Option<(SocketAddr, [u8; 44])>,
    ) -> Result<Approval, OwnerError> {
        let (approved, staged) = self
            .authorize_phone(session, originals, proof, &mut mint, endpoint)
            .await?;
        // Canonical expiry owns first-cause and cleanup before a mutating resume.
        // Read-only status/recovery never take this path. Later stage/publication
        // already obtain new admissions after an expiry changes the revision.
        let current = self.expire(approved.id()).await?;
        // An exact replay of a stored stage or an active receipt draws nothing.
        // A first commit already minted its stage. Only an approved record that
        // still has none asks the generator, and only for that stage.
        let stage = if current.phase() == PairingPhase::Approved {
            Some(match staged {
                Some(stage) => stage,
                None => mint()?.1,
            })
        } else {
            None
        };
        self.activate_approved(session, current, stage).await
    }
    /// Admit the exact stored original before exposing it. A lookup is never admission.
    pub async fn phone_status(
        &self,
        session: &AuthenticatedSession,
        originals: &dyn OwnerPhoneStore,
        source: &OwnerPhoneSource,
        request: OwnerPhoneRequestId,
        digest: &[u8; 32],
    ) -> Result<OwnerPhoneOriginalReceipt, OwnerError> {
        let prior = originals
            .find_owner_phone_original(source, request)
            .map_err(OwnerError::Enrollment)?;
        let admission = self
            .authorization
            .execute(session, prior.record().intent(), self.gateway)
            .await
            .map_err(OwnerError::Authorization)?;
        let receipt = originals
            .read_owner_phone_original(prior.original(), &admission, self.clock)
            .map_err(OwnerError::Enrollment)?;
        if receipt.digest() != digest {
            return Err(OwnerError::Domain(PairingError::Conflict));
        }
        Ok(receipt)
    }

    /// Commit one verified original. Concurrent equal first calls converge only by
    /// re-reading and freshly admitting the already committed intent.
    pub async fn authorize_phone(
        &self,
        session: &AuthenticatedSession,
        originals: &dyn OwnerPhoneStore,
        proof: &VerifiedOwnerPhoneDeviceProof,
        mint: &mut impl FnMut() -> Result<(FreshOwnerPhone, FreshStage), OwnerError>,
        endpoint: Option<(SocketAddr, [u8; 44])>,
    ) -> Result<(PairingRecord, Option<FreshStage>), OwnerError> {
        let request = proof.request();
        let mut race_failure = None;
        // Kept across a lost race so the winner's later activation can use the
        // identities this call already drew, instead of drawing again.
        let mut staged = None;
        loop {
            let initial = match originals.find_owner_phone_original(request.source(), request.id())
            {
                Ok(prior) => {
                    if prior.original() != request || prior.digest() != proof.digest() {
                        return Err(OwnerError::Domain(PairingError::Conflict));
                    }
                    prior.record().clone()
                }
                Err(PairingStoreError::NotFound) if race_failure.is_none() => {
                    if endpoint != Some((request.endpoint(), *request.gateway_pin())) {
                        return Err(OwnerError::Domain(PairingError::Conflict));
                    }
                    let (fresh, stage) = mint()?;
                    staged = Some(stage);
                    let source = request.source();
                    let intent = ConsentIntent::new(
                        fresh.consent,
                        1,
                        source.audience().clone(),
                        source.owner().clone(),
                        source.membership().clone(),
                        self.gateway.clone(),
                        ConsentClass::OwnerPhoneReadWrite,
                    )
                    .map_err(OwnerError::Domain)?;
                    PairingRecord::new_owner_phone(
                        fresh.invitation,
                        intent,
                        self.clock.unix_milliseconds(),
                        self.policy,
                    )
                    .map_err(OwnerError::Domain)?
                }
                Err(PairingStoreError::NotFound) => {
                    return Err(OwnerError::Enrollment(
                        race_failure.unwrap_or(PairingStoreError::NotFound),
                    ))
                }
                Err(error) => return Err(OwnerError::Enrollment(error)),
            };
            let admission = self
                .authorization
                .execute(session, initial.intent(), self.gateway)
                .await
                .map_err(OwnerError::Authorization)?;
            match originals
                .authorize_owner_phone(&initial, proof, &admission, self.clock)
                .await
            {
                Ok(record) => return Ok((record, staged)),
                Err(
                    error @ (PairingStoreError::Domain(PairingError::Conflict)
                    | PairingStoreError::StaleRevision),
                ) if race_failure.is_none() => race_failure = Some(error),
                Err(error) => return Err(OwnerError::Enrollment(error)),
            }
        }
    }
}
