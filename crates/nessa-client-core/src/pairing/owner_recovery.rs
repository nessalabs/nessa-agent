//! Pinned original recovery and explicit same-private-owner settlement.
//! Public receipts retain facts, and the physical client owns all network work.
use super::owner_phone::{proof, validate, OwnerPhonePreparation, OwnerPhonePreparationError};
use super::{NativeClientError, NativeEnrollmentClient};
use nessa_auth::{
    adapters::pairing::{GatewayTrust, NativeIdentity, NativeTransport},
    application::pairing::{
        ClientPendingStore, DeviceCredential, OwnerPhoneAdoption, OwnerPhoneAdoptionFailure,
        OwnerPhoneEnrollmentKind, OwnerPhoneEnrollmentStore, OwnerPhoneOriginalStore,
        PendingEnrollment, PrivateKeyMaterial, PrivateStateError,
    },
    domain::pairing::{DisclosedConsent, PairingError, PublicIntent},
};
use nessa_protocol::pairing::{
    socket::{begin_enrollment_phase, check_deadline, DeadlineStream, NativeDeadline},
    wire::{NativePairingReply, NativePairingRequest, NativePairingStatus, NativeWireError},
    EnrollmentChannel,
};
use std::{net::TcpStream, sync::Arc};

/// One injected private owner, never independent original and enrollment stores.
pub trait OwnerPhoneState:
    ClientPendingStore + OwnerPhoneOriginalStore + OwnerPhoneEnrollmentStore
{
}
impl<T: ClientPendingStore + OwnerPhoneOriginalStore + OwnerPhoneEnrollmentStore> OwnerPhoneState
    for T
{
}

/// Exact preparation or canonical physical client refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OwnerPhoneRecoveryFailure {
    /// Actual original restoration or canonical proof refused.
    Preparation(OwnerPhonePreparationError),
    /// Physical admission, handshake, bounded exchange or worker refused.
    Native(NativeClientError),
}
impl From<NativeClientError> for OwnerPhoneRecoveryFailure {
    fn from(value: NativeClientError) -> Self {
        Self::Native(value)
    }
}
fn conflict() -> NativeClientError {
    NativeClientError::Wire(NativeWireError::Correlation(PairingError::Conflict))
}

/// Only the actual pinned worker constructs this public-only historical receipt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerPhoneRecovered {
    preparation: OwnerPhonePreparation,
    status: NativePairingStatus,
}
impl OwnerPhoneRecovered {
    /// Exact public proof reconstructed from the original private owner.
    pub fn preparation(&self) -> &OwnerPhonePreparation {
        &self.preparation
    }
    /// Actual historical pinned status; it does not admit future requests.
    pub fn status(&self) -> &NativePairingStatus {
        &self.status
    }
}

/// Typed factual settlement; this never publishes current command authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OwnerPhoneSettlement {
    /// Exact acknowledged local receipt correlated to the actual saved record.
    Adopted(OwnerPhoneAdoption),
    /// Existing terminal end owner completed exact enrollment removal.
    Ended,
}
/// An original producer failure or a separately retained reported success/refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OwnerPhoneSettlementFailure {
    /// Current original restoration refused before settlement.
    Preparation(OwnerPhonePreparationError),
    /// Original Auth phase, private failure and independent earlier acknowledgement.
    Adoption(OwnerPhoneAdoptionFailure),
    /// Reported success contradicted actual original or enrollment selection.
    SavedRecord {
        /// The producer's original nominal receipt, retained as a reported fact.
        reported: OwnerPhoneAdoption,
        /// Actual subsequent correlation/read/crypto failure.
        error: OwnerPhoneRecoveryFailure,
    },
    /// Authenticated terminal private removal failed after its separate purge attempt.
    End(PrivateStateError),
    /// Actual terminal record did not match the original observation.
    TerminalRecord(NativeClientError),
    /// Current immutable bound reference did not match this recovered public reference.
    Correlation(NativeClientError),
}

// This internal value never escapes the physical work closure.
struct RestoredOwner {
    identity: NativeIdentity,
    preparation: OwnerPhonePreparation,
    bound: Option<PublicIntent>,
}
fn restore_owner(
    state: &dyn OwnerPhoneState,
    expected: &OwnerPhonePreparation,
) -> Result<RestoredOwner, OwnerPhonePreparationError> {
    let saved = state
        .load_original()
        .map_err(OwnerPhonePreparationError::Private)?
        .ok_or(OwnerPhonePreparationError::Missing)?;
    let bound = saved.public_intent();
    let (reservation, key) = saved.into_parts();
    validate(&reservation)?;
    let key = key.ok_or(OwnerPhonePreparationError::Incomplete)?;
    let identity = NativeIdentity::restore(key).map_err(OwnerPhonePreparationError::Crypto)?;
    // Reuse canonical preparation's proof function; do not reimplement signing/digest.
    let preparation = proof(&reservation, &identity)?;
    if preparation.original() != expected.original()
        || preparation.signature() != expected.signature()
        || preparation.digest() != expected.digest()
    {
        return Err(OwnerPhonePreparationError::Private(
            PrivateStateError::Conflict,
        ));
    }
    Ok(RestoredOwner {
        identity,
        preparation,
        bound,
    })
}

impl NativeEnrollmentClient {
    // The retained combined-owner composition alone calls this shared worker seam.
    // Existing dispatcher/helpers and the clock field become pub(super), not a
    // public callback or clock API. This impl belongs to the same pairing feature.
    pub(crate) async fn owner_recovery_completed<T: Send + 'static>(
        &self,
        stream: TcpStream,
        state: Arc<dyn OwnerPhoneState>,
        expected: Arc<OwnerPhonePreparation>,
        received: Option<DisclosedConsent>,
        begin: impl FnOnce() + Send + 'static,
        complete: impl FnOnce(
                Result<OwnerPhoneRecovered, OwnerPhoneRecoveryFailure>,
                &dyn ClientPendingStore,
                &dyn OwnerPhoneState,
            ) -> T
            + Send
            + 'static,
    ) -> Result<T, NativeClientError> {
        let endpoint = super::client::client_endpoint(&stream)?;
        let clock = self.clock.clone();
        self.dispatch_owned(vec![endpoint.clone()], move |pending| {
            begin();
            let result = (|| {
                let restored = restore_owner(state.as_ref(), &expected)
                    .map_err(OwnerPhoneRecoveryFailure::Preparation)?;
                if stream
                    .peer_addr()
                    .map_err(|error| NativeClientError::Io(error.kind()))?
                    != restored.preparation.original().endpoint()
                {
                    return Err(OwnerPhoneRecoveryFailure::Native(conflict()));
                }
                let (stream, deadline) = DeadlineStream::new(stream, clock, endpoint)
                    .map_err(super::client::physical_error)?;
                stream.blocking().map_err(super::client::physical_error)?;
                let mut observed = None;
                let channel = NativeTransport::connect_observed(
                    stream,
                    &restored.identity,
                    GatewayTrust::Pinned(*restored.preparation.original().gateway_pin()),
                    |error| observed = Some(NativeClientError::Handshake(error)),
                )
                .map_err(|error| observed.unwrap_or(NativeClientError::Handshake(error)))?;
                let mut channel = EnrollmentChannel::new(channel);
                begin_enrollment_phase(&deadline).map_err(super::client::physical_error)?;
                let status = exchange(&mut channel, &deadline, &restored, received.as_ref())?;
                Ok(OwnerPhoneRecovered {
                    preparation: restored.preparation,
                    status,
                })
            })();
            // The physical task owns both the same-state Arc and all factual completion.
            Ok(complete(result, pending, state.as_ref()))
        })
        .await
    }
}

fn exchange(
    channel: &mut EnrollmentChannel<DeadlineStream>,
    deadline: &NativeDeadline,
    restored: &RestoredOwner,
    received: Option<&DisclosedConsent>,
) -> Result<NativePairingStatus, NativeClientError> {
    let original = restored.preparation.original();
    super::client::send(
        channel,
        NativePairingRequest::RecoverOwnerPhone {
            request_id: original.id(),
            digest: *restored.preparation.digest(),
        },
    )?;
    let NativePairingReply::OwnerPhoneRecovery {
        request_id,
        digest,
        status,
    } = super::client::receive(channel)?
    else {
        return Err(NativeClientError::Phase);
    };
    if request_id != original.id() || &digest != restored.preparation.digest() {
        return Err(conflict());
    }
    let consent = status.consent().ok_or(NativeClientError::Phase)?;
    original
        .correlate_disclosure(consent)
        .map_err(|error| NativeClientError::Wire(NativeWireError::Correlation(error)))?;
    status
        .correlate(restored.bound.unwrap_or_else(|| status.public()), received)
        .map_err(NativeClientError::Wire)?;
    check_deadline(deadline).map_err(super::client::physical_error)?;
    Ok(status)
}

// A failed Result<Recovered, ...> cannot call this: it requires the private-constructor
// recovered type. Retained completion matches Ok before invoking settlement.
pub(crate) fn settle(
    recovered: &OwnerPhoneRecovered,
    state: &dyn OwnerPhoneState,
    ending: &dyn ClientPendingStore,
) -> Result<OwnerPhoneSettlement, OwnerPhoneSettlementFailure> {
    let restored = restore_owner(state, &recovered.preparation)
        .map_err(OwnerPhoneSettlementFailure::Preparation)?;
    let status = &recovered.status;
    if restored.bound.is_some_and(|bound| bound != status.public()) {
        return Err(OwnerPhoneSettlementFailure::Correlation(conflict()));
    }
    let consent = status
        .consent()
        .expect("recovery requires disclosed claimed status");
    if matches!(status, NativePairingStatus::Terminal { .. }) {
        check_terminal_record(state, recovered)
            .map_err(OwnerPhoneSettlementFailure::TerminalRecord)?;
        ending
            .end_enrollment(status.public())
            .map_err(OwnerPhoneSettlementFailure::End)?;
        return Ok(OwnerPhoneSettlement::Ended);
    }
    let issued = match status {
        NativePairingStatus::Active {
            credential,
            receiver,
            ..
        } => Some((credential, receiver)),
        NativePairingStatus::Claimed(_)
        | NativePairingStatus::Approved(_)
        | NativePairingStatus::Staging(_) => None,
        _ => unreachable!("recovery requires disclosed claimed status"),
    };
    let reported = state
        .adopt_owner_phone(
            recovered.preparation.original(),
            recovered.preparation.digest(),
            consent,
            issued,
        )
        .map_err(OwnerPhoneSettlementFailure::Adoption)?;
    let correlation = (|| {
        let expected_kind = if issued.is_some() {
            OwnerPhoneEnrollmentKind::Issued
        } else {
            OwnerPhoneEnrollmentKind::Pending
        };
        if reported.public() != status.public() || reported.kind() != expected_kind {
            return Err(OwnerPhoneRecoveryFailure::Native(conflict()));
        }
        // An injected nominal success must have actually bound the SAME original.
        let original = restore_owner(state, &recovered.preparation)
            .map_err(OwnerPhoneRecoveryFailure::Preparation)?;
        if original.bound != Some(status.public()) {
            return Err(OwnerPhoneRecoveryFailure::Native(conflict()));
        }
        correlate_saved(state, recovered, expected_kind).map_err(OwnerPhoneRecoveryFailure::Native)
    })();
    correlation.map_err(|error| OwnerPhoneSettlementFailure::SavedRecord { reported, error })?;
    Ok(OwnerPhoneSettlement::Adopted(reported))
}

fn correlate_key_pin_public(
    key: &PrivateKeyMaterial,
    pin: &[u8; 44],
    public: PublicIntent,
    recovered: &OwnerPhoneRecovered,
) -> Result<(), NativeClientError> {
    // This private bounded zeroizing copy stays native; Auth owns key derivation.
    let identity = NativeIdentity::restore(PrivateKeyMaterial::new(zeroize::Zeroizing::new(
        *key.expose_bytes(),
    )))
    .map_err(NativeClientError::Crypto)?;
    let original = recovered.preparation.original();
    if identity.device_key() != original.device_key()
        || pin != original.gateway_pin()
        || public != recovered.status.public()
    {
        return Err(conflict());
    }
    Ok(())
}
fn correlate_enrollment(
    saved: &PendingEnrollment,
    recovered: &OwnerPhoneRecovered,
) -> Result<(), NativeClientError> {
    correlate_key_pin_public(saved.key(), saved.gateway_pin(), saved.intent(), recovered)
}
/// Reload the actual selected issued record and correlate the complete owned recovery.
/// Callers must use this same returned record, not reload another record for selection.
/// This publishes no current authority; subsequent gateway admission remains independent.
pub fn issued_owner_phone(
    state: &dyn OwnerPhoneState,
    recovered: &OwnerPhoneRecovered,
) -> Result<DeviceCredential, NativeClientError> {
    let pending = state.load_pending().map_err(NativeClientError::Storage)?;
    let issued = state
        .load_credential()
        .map_err(NativeClientError::Storage)?
        .ok_or(NativeClientError::NoPending)?;
    if pending.is_some() {
        return Err(conflict());
    }
    let NativePairingStatus::Active {
        credential,
        receiver,
        ..
    } = recovered.status()
    else {
        return Err(conflict());
    };
    if issued.credential() != credential || issued.receiver() != receiver {
        return Err(conflict());
    }
    correlate_key_pin_public(
        issued.key(),
        issued.gateway_pin(),
        issued.intent(),
        recovered,
    )?;
    Ok(issued)
}
fn correlate_saved(
    state: &dyn OwnerPhoneState,
    recovered: &OwnerPhoneRecovered,
    kind: OwnerPhoneEnrollmentKind,
) -> Result<(), NativeClientError> {
    match kind {
        OwnerPhoneEnrollmentKind::Issued => {
            issued_owner_phone(state, recovered)?;
            Ok(())
        }
        OwnerPhoneEnrollmentKind::Pending => {
            let pending = state.load_pending().map_err(NativeClientError::Storage)?;
            let issued = state
                .load_credential()
                .map_err(NativeClientError::Storage)?;
            match (pending, issued) {
                (Some(pending), None) => correlate_enrollment(&pending, recovered),
                _ => Err(conflict()),
            }
        }
    }
}

fn check_terminal_record(
    state: &dyn OwnerPhoneState,
    recovered: &OwnerPhoneRecovered,
) -> Result<(), NativeClientError> {
    let pending = state.load_pending().map_err(NativeClientError::Storage)?;
    let issued = state
        .load_credential()
        .map_err(NativeClientError::Storage)?;
    match (pending, issued) {
        (None, None) => Ok(()),
        (Some(pending), None) => correlate_enrollment(&pending, recovered),
        (None, Some(issued)) => correlate_enrollment(&issued.into_enrollment(), recovered),
        _ => Err(conflict()),
    }
}

/// Restore the current original without initial-key fallback or network contact.
/// None names a read-only, code-redeemed or empty scope; unreadable state refuses.
pub fn current_owner_phone(
    state: &dyn OwnerPhoneState,
) -> Result<Option<OwnerPhonePreparation>, OwnerPhonePreparationError> {
    let Some(saved) = state
        .load_original()
        .map_err(OwnerPhonePreparationError::Private)?
    else {
        // A signed enrollment whose original disappeared never falls back to
        // the code path; an owner phone redeemed by code never had one.
        if state
            .enrollment_signed()
            .map_err(OwnerPhonePreparationError::Private)?
        {
            return Err(OwnerPhonePreparationError::Missing);
        }
        return Ok(None);
    };
    let (reservation, key) = saved.into_parts();
    validate(&reservation)?;
    let identity = NativeIdentity::restore(key.ok_or(OwnerPhonePreparationError::Incomplete)?)
        .map_err(OwnerPhonePreparationError::Crypto)?;
    proof(&reservation, &identity).map(Some)
}

/// Restore exactly an escaped public proof; a missing original never prepares anew.
pub fn restore_expected_owner_phone(
    state: &dyn OwnerPhoneState,
    expected: &OwnerPhonePreparation,
) -> Result<OwnerPhonePreparation, OwnerPhonePreparationError> {
    restore_owner(state, expected).map(|value| value.preparation)
}
