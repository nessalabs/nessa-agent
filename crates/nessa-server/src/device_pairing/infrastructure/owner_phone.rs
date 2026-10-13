//! Signed original commands keep the same blocking lease through activation.
use super::{GatewayPairing, PairingRuntimeError};
use crate::device_pairing::application::{
    claimed_status, ActivationError, DeviceStatusError, FreshOwnerPhone, FreshStage,
    PairingReceivers, ReceiverError,
};
use nessa_auth::{
    adapters::pairing::{verify_owner_phone_request, CryptoRng, RngCore},
    application::{pairing::OwnerPhoneOriginalReceipt, session::AuthenticatedSession},
    domain::{
        pairing::{
            AttemptId, ConsentIntentId, InvitationId, OwnerPhoneRequest, OwnerPhoneRequestId,
            OwnerPhoneSource,
        },
        CredentialId,
    },
};

/// Complete historical original with the independently known activation stop.
pub struct PhoneAuthorization {
    /// Same-entry signed input and its restored enrollment history.
    pub original: OwnerPhoneOriginalReceipt,
    /// Actual activation stop; independent of whether the approval committed.
    pub stopped: Option<ActivationError>,
    /// Current epoch from the original receiver's actual holding, for Active only.
    pub access_epoch: Option<u64>,
}

fn current(
    original: OwnerPhoneOriginalReceipt,
    stopped: Option<ActivationError>,
    receivers: &dyn PairingReceivers,
) -> Result<PhoneAuthorization, ReceiverError> {
    let status =
        claimed_status(original.record().clone(), receivers).map_err(|error| match error {
            DeviceStatusError::Receiver(error) => error,
            _ => ReceiverError::Conflict,
        })?;
    let access_epoch = match status {
        nessa_protocol::pairing::DevicePairingStatus::Active { access_epoch, .. } => {
            Some(access_epoch)
        }
        _ => None,
    };
    Ok(PhoneAuthorization {
        original,
        stopped,
        access_epoch,
    })
}

impl GatewayPairing {
    /// Verify the exact signed request before any registry mutation, then own
    /// approval, receiver settlement and final receipt independently of observers.
    pub async fn authorize_phone<R: RngCore + CryptoRng + Send + 'static>(
        &self,
        session: &AuthenticatedSession,
        request: OwnerPhoneRequest,
        signature: [u8; 64],
        mut entropy: R,
    ) -> Result<PhoneAuthorization, PairingRuntimeError> {
        let proof =
            verify_owner_phone_request(request, &signature).map_err(PairingRuntimeError::Crypto)?;
        let mut bytes = [0; 64];
        entropy
            .try_fill_bytes(&mut bytes)
            .map_err(|_| PairingRuntimeError::Entropy)?;
        let mut invitation = [0; 16];
        invitation.copy_from_slice(&bytes[..16]);
        let mut consent = [0; 16];
        consent.copy_from_slice(&bytes[16..32]);
        let mut stage = [0; 16];
        stage.copy_from_slice(&bytes[48..]);
        let fresh = FreshOwnerPhone {
            invitation: InvitationId::new(invitation),
            consent: ConsentIntentId::new(consent),
        };
        let stage = FreshStage {
            credential: CredentialId::new(format!("device-{}", super::hex(&bytes[32..48])))
                .map_err(|_| PairingRuntimeError::Entropy)?,
            request: AttemptId::new(stage),
        };
        let originals = self.dependencies.originals.clone();
        let endpoint = self.native_endpoint();
        let session = session.clone();
        self.owner_command(move |owner, handle| {
            handle.block_on(async {
                let approval = owner
                    .authorize_and_activate_phone(
                        &session,
                        originals.as_ref(),
                        &proof,
                        fresh,
                        stage,
                        endpoint,
                    )
                    .await?;
                let original = owner
                    .phone_status(
                        &session,
                        originals.as_ref(),
                        proof.request().source(),
                        proof.request().id(),
                        proof.digest(),
                    )
                    .await?;
                Ok(current(original, approval.stopped, owner.receivers))
            })
        })
        .await?
        .map_err(PairingRuntimeError::Receiver)
    }

    /// Same-original product observation requires fresh original bootstrap admission.
    pub async fn phone_status(
        &self,
        session: &AuthenticatedSession,
        source: OwnerPhoneSource,
        request: OwnerPhoneRequestId,
        digest: [u8; 32],
    ) -> Result<PhoneAuthorization, PairingRuntimeError> {
        let session = session.clone();
        let originals = self.dependencies.originals.clone();
        self.owner_command(move |owner, handle| {
            handle.block_on(async {
                let original = owner
                    .phone_status(&session, originals.as_ref(), &source, request, &digest)
                    .await?;
                Ok(current(original, None, owner.receivers))
            })
        })
        .await?
        .map_err(PairingRuntimeError::Receiver)
    }
}
