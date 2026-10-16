//! Immutable original proof over canonical preparation and representation owners.
use crate::{
    failures::{NativeCryptoFailure, NativePrivateStateFailure, NativeSessionFailure},
    serialization::{bounded_json, SerializationFailure},
    session::SessionPreparationFailure,
};
use nessa_auth::{
    application::pairing::OwnerPhoneReservation, domain::pairing::OwnerPhoneRequestId,
};
use nessa_client_core::pairing::{OwnerPhonePreparation, OwnerPhonePreparationError};
use nessa_protocol::{
    pairing::MAX_PROTECTED_RESPONSE_BYTES,
    product::{
        generated::PhoneDiscovery,
        owner_phone::{self, PhoneDtoError},
    },
    protocol::unique_value,
};
use std::{
    error::Error,
    fmt::{Display, Formatter, Result as FormatResult},
    sync::Arc,
};

/// Representation refusals; none authorizes preparation or changes a saved proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum NativeOwnerArgumentFailure {
    /// Input exceeds the published protected response bound.
    TooLarge,
    /// Malformed, duplicate, over-budget or incompatible discovery JSON.
    Json,
    /// Original UUID bytes have invalid width or value.
    RequestId,
    /// Existing canonical conversion rejected an array width.
    Length,
    /// Existing canonical conversion rejected a non-octet integer.
    Octet,
    /// Existing canonical conversion rejected source identity/resource linkage.
    Source,
    /// Existing canonical conversion rejected the numeric destination.
    Endpoint,
    /// Existing canonical conversion rejected original correlation.
    Original,
    /// Existing canonical conversion rejected complete consent correspondence.
    Consent,
    /// A bounded public JSON copy exceeded its published limit.
    CopyTooLarge,
    /// A public JSON copy could not represent its original value.
    CopyInvalid,
}
impl From<PhoneDtoError> for NativeOwnerArgumentFailure {
    fn from(value: PhoneDtoError) -> Self {
        match value {
            PhoneDtoError::Length => Self::Length,
            PhoneDtoError::Octet => Self::Octet,
            PhoneDtoError::Source => Self::Source,
            PhoneDtoError::Endpoint => Self::Endpoint,
            PhoneDtoError::Original => Self::Original,
            PhoneDtoError::Consent => Self::Consent,
        }
    }
}
/// Existing canonical original preparation refusal family.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum NativeOwnerPreparationFailureKind {
    /// Existing crypto owner refused.
    Crypto,
    /// Original private publication/read refused.
    Private,
    /// Existing reservation has no native original history.
    Missing,
    /// Original was reserved but no seed was durably prepared.
    Incomplete,
}
/// Original typed payload, with no private bytes or readiness meaning.
#[derive(Debug, uniffi::Object)]
pub struct NativeOwnerPreparationFailure {
    pub(crate) value: OwnerPhonePreparationError,
}
#[uniffi::export]
impl NativeOwnerPreparationFailure {
    /// Original family.
    pub fn kind(&self) -> NativeOwnerPreparationFailureKind {
        match self.value {
            OwnerPhonePreparationError::Crypto(_) => NativeOwnerPreparationFailureKind::Crypto,
            OwnerPhonePreparationError::Private(_) => NativeOwnerPreparationFailureKind::Private,
            OwnerPhonePreparationError::Missing => NativeOwnerPreparationFailureKind::Missing,
            OwnerPhonePreparationError::Incomplete => NativeOwnerPreparationFailureKind::Incomplete,
        }
    }
    /// Original selected crypto refusal.
    pub fn crypto(&self) -> Option<NativeCryptoFailure> {
        match self.value {
            OwnerPhonePreparationError::Crypto(value) => Some(value.into()),
            _ => None,
        }
    }
    /// Actual private-state error, including original publication/cleanup/reconciliation.
    pub fn private_state(&self) -> Option<Arc<NativePrivateStateFailure>> {
        match self.value {
            OwnerPhonePreparationError::Private(value) => {
                Some(Arc::new(NativePrivateStateFailure::from_state(value)))
            }
            _ => None,
        }
    }
}
/// Foreign refusal keeps representation, parent lifecycle and preparation distinct.
#[derive(Debug, uniffi::Error)]
pub enum NativeOwnerPreparationCallError {
    /// Argument or bounded getter copy failed, without changing native facts.
    Argument {
        /// Existing representation refusal.
        failure: NativeOwnerArgumentFailure,
    },
    /// Original parent admission/lifecycle failure.
    Session {
        /// Original typed payload.
        failure: Arc<NativeSessionFailure>,
    },
    /// Original preparation/publication refusal.
    Preparation {
        /// Original typed payload.
        failure: Arc<NativeOwnerPreparationFailure>,
    },
}
impl From<SessionPreparationFailure> for NativeOwnerPreparationCallError {
    fn from(value: SessionPreparationFailure) -> Self {
        match value {
            SessionPreparationFailure::Session(value) => Self::Session {
                failure: Arc::new(NativeSessionFailure { value }),
            },
            SessionPreparationFailure::Preparation(value) => Self::Preparation {
                failure: Arc::new(NativeOwnerPreparationFailure { value }),
            },
        }
    }
}
impl From<NativeOwnerArgumentFailure> for NativeOwnerPreparationCallError {
    fn from(failure: NativeOwnerArgumentFailure) -> Self {
        Self::Argument { failure }
    }
}
impl Display for NativeOwnerPreparationCallError {
    fn fmt(&self, output: &mut Formatter<'_>) -> FormatResult {
        write!(output, "{self:?}")
    }
}
impl Error for NativeOwnerPreparationCallError {}

pub(crate) fn reservation(
    request_id: Vec<u8>,
    discovery_json: String,
) -> Result<OwnerPhoneReservation, NativeOwnerPreparationCallError> {
    if discovery_json.len() > MAX_PROTECTED_RESPONSE_BYTES {
        return Err(NativeOwnerArgumentFailure::TooLarge.into());
    }
    let id = request_id
        .try_into()
        .map_err(|_| NativeOwnerArgumentFailure::RequestId)?;
    let id = OwnerPhoneRequestId::new(id).map_err(|_| NativeOwnerArgumentFailure::RequestId)?;
    let value = unique_value(&discovery_json).map_err(|_| NativeOwnerArgumentFailure::Json)?;
    let discovery: PhoneDiscovery =
        serde_json::from_value(value).map_err(|_| NativeOwnerArgumentFailure::Json)?;
    let context = owner_phone::context(discovery).map_err(NativeOwnerArgumentFailure::from)?;
    Ok(OwnerPhoneReservation::new(id, context))
}
/// Immutable signed historical original. It grants no remote or local authority.
#[derive(uniffi::Object)]
pub struct NativeOwnerPhonePreparation {
    pub(crate) value: OwnerPhonePreparation,
}
#[uniffi::export]
impl NativeOwnerPhonePreparation {
    /// Complete canonical public original, bounded before native copy growth.
    pub fn original_json(&self) -> Result<String, NativeOwnerPreparationCallError> {
        let original = owner_phone::original(self.value.original())
            .map_err(NativeOwnerArgumentFailure::from)?;
        bounded_json(&original, MAX_PROTECTED_RESPONSE_BYTES).map_err(|error| {
            NativeOwnerPreparationCallError::from(match error {
                SerializationFailure::TooLarge => NativeOwnerArgumentFailure::CopyTooLarge,
                SerializationFailure::Invalid => NativeOwnerArgumentFailure::CopyInvalid,
            })
        })
    }
    /// Exact original UUID bytes.
    pub fn request_id(&self) -> Vec<u8> {
        self.value.original().id().bytes().to_vec()
    }
    /// Actual permanent public device key, never a bearer or private seed.
    pub fn device_key(&self) -> Vec<u8> {
        self.value.original().device_key().bytes().to_vec()
    }
    /// Signature produced by the same canonical native crypto owner.
    pub fn signature(&self) -> Vec<u8> {
        self.value.signature().to_vec()
    }
    /// Digest computed by that owner's proof verifier.
    pub fn digest(&self) -> Vec<u8> {
        self.value.digest().to_vec()
    }
}

#[cfg(test)]
#[path = "../tests/session/owner_proof.rs"]
mod tests;
