//! Original reservation and seed share FilePairingState's private lock and publisher.
use super::{decode_public, encode_public, ClientRecord, FilePairingState, PUBLIC_INTENT_BYTES};
use crate::{
    adapters::pairing::{relay_key, NativeIdentity},
    application::pairing::{
        OwnerPhoneOriginalCreation, OwnerPhoneOriginalStore, OwnerPhonePrivateOriginal,
        OwnerPhoneReservation, PrivateKeyMaterial, PrivateStateError,
    },
    domain::{
        pairing::{
            ConsentClass, DeviceKey, OwnerPhoneContext, OwnerPhoneRequestId, OwnerPhoneSource,
            PublicIntent,
        },
        AudienceId, CredentialId, MembershipId, OrganizationId, PrincipalId, Resource, ResourceId,
        MAX_IDENTIFIER_BYTES,
    },
};
use std::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    sync::MutexGuard,
};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;
pub(super) const ORIGINAL_FILE: &str = "owner-phone-original";
const RESERVED: &[u8; 4] = b"NSOR";
const PREPARED: &[u8; 4] = b"NSOK";
// Six bounded identifiers, UUID, pin, largest numeric endpoint and optional seed.
const MAX_BYTES: usize = 4
    + 16
    + 6 * (2 + MAX_IDENTIFIER_BYTES)
    + 44
    + 1
    + 16
    + 2
    + 4
    + 4
    + 32
    + DeviceKey::LENGTH
    + 1
    + PUBLIC_INTENT_BYTES;
struct Creation<'a> {
    owner: &'a FilePairingState,
    _operation: MutexGuard<'a, ()>,
    reservation: OwnerPhoneReservation,
}
impl OwnerPhoneOriginalCreation for Creation<'_> {
    fn reservation(&self) -> &OwnerPhoneReservation {
        &self.reservation
    }
    fn prepare(self: Box<Self>, key: &PrivateKeyMaterial) -> Result<(), PrivateStateError> {
        let saved = self
            .owner
            .load_original_record()?
            .ok_or(PrivateStateError::Conflict)?;
        if saved.reservation() != &self.reservation || saved.key().is_some() {
            return Err(PrivateStateError::Conflict);
        }
        let bytes = encode(&self.reservation, Some(key), None)?;
        self.owner.publish(ORIGINAL_FILE, &bytes, true)
    }
}
impl FilePairingState {
    pub(super) fn load_original_record(
        &self,
    ) -> Result<Option<OwnerPhonePrivateOriginal>, PrivateStateError> {
        self.read_bounded(ORIGINAL_FILE, MAX_BYTES)?
            .map(|bytes| decode(&bytes))
            .transpose()
    }
    pub(super) fn exclude_original(&self) -> Result<(), PrivateStateError> {
        if self.load_original_record()?.is_some() {
            Err(PrivateStateError::Conflict)
        } else {
            Ok(())
        }
    }
    pub(super) fn checked_owner_phone_enrollment(
        original: &OwnerPhonePrivateOriginal,
        enrollment: &ClientRecord,
    ) -> Result<Zeroizing<Vec<u8>>, PrivateStateError> {
        let (key, pin, public) = match enrollment {
            ClientRecord::Pending(saved) => (saved.key(), saved.gateway_pin(), saved.intent()),
            ClientRecord::Credential(saved) => (saved.key(), saved.gateway_pin(), saved.intent()),
        };
        let original_key = original.key().ok_or(PrivateStateError::Conflict)?;
        if original.public_intent() != Some(public)
            || original.reservation().context().gateway_pin() != pin
            || !bool::from(original_key.expose_bytes().ct_eq(key.expose_bytes()))
        {
            return Err(PrivateStateError::Conflict);
        }
        encode(original.reservation(), Some(original_key), Some(public))
    }
}
impl OwnerPhoneOriginalStore for FilePairingState {
    fn reserve_original(
        &self,
        reservation: OwnerPhoneReservation,
    ) -> Result<Box<dyn OwnerPhoneOriginalCreation + '_>, PrivateStateError> {
        if relay_key(reservation.context().gateway_pin()).is_none() {
            return Err(PrivateStateError::Corrupt);
        }
        let operation = self
            .operation
            .lock()
            .map_err(|_| PrivateStateError::Unavailable)?;
        if self.load_original_record()?.is_some() || self.load_client()?.is_some() {
            return Err(PrivateStateError::Conflict);
        }
        let bytes = encode(&reservation, None, None)?;
        self.publish(ORIGINAL_FILE, &bytes, false)?;
        Ok(Box::new(Creation {
            owner: self,
            _operation: operation,
            reservation,
        }))
    }
    fn load_original(&self) -> Result<Option<OwnerPhonePrivateOriginal>, PrivateStateError> {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| PrivateStateError::Unavailable)?;
        // The existing enrollment owner also rejects mixed original/enrollment files.
        self.load_client()?;
        let Some(bytes) = self.read_bounded(ORIGINAL_FILE, MAX_BYTES)? else {
            return Ok(None);
        };
        let saved = decode(&bytes)?;
        // Current file bytes alone are not a durable acknowledgement. The existing
        // owner verifies its named handle, fsyncs the file/directory and retains typed refusal.
        self.reconcile(ORIGINAL_FILE, &bytes)?;
        Ok(Some(saved))
    }
    fn enrollment_signed(&self) -> Result<bool, PrivateStateError> {
        FilePairingState::enrollment_signed(self)
    }
}
fn encode(
    reservation: &OwnerPhoneReservation,
    key: Option<&PrivateKeyMaterial>,
    public: Option<PublicIntent>,
) -> Result<Zeroizing<Vec<u8>>, PrivateStateError> {
    if public
        .is_some_and(|public| key.is_none() || public.class() != ConsentClass::OwnerPhoneReadWrite)
    {
        return Err(PrivateStateError::Corrupt);
    }
    let mut bytes = Zeroizing::new(Vec::with_capacity(MAX_BYTES));
    bytes.extend_from_slice(if key.is_some() { PREPARED } else { RESERVED });
    bytes.extend_from_slice(reservation.id().bytes());
    let context = reservation.context();
    let source = context.source();
    for value in [
        source.owner().as_str(),
        source.membership().as_str(),
        source.audience().as_str(),
        source.credential().as_str(),
        source.resource().organization_id().as_str(),
        source.resource().id().as_str(),
    ] {
        bytes.extend_from_slice(&(value.len() as u16).to_be_bytes());
        bytes.extend_from_slice(value.as_bytes());
    }
    bytes.extend_from_slice(context.gateway_pin());
    match context.endpoint() {
        SocketAddr::V4(address) => {
            bytes.push(4);
            bytes.extend_from_slice(&address.ip().octets());
            bytes.extend_from_slice(&address.port().to_be_bytes());
        }
        SocketAddr::V6(address) => {
            bytes.push(6);
            bytes.extend_from_slice(&address.ip().octets());
            bytes.extend_from_slice(&address.port().to_be_bytes());
            bytes.extend_from_slice(&address.flowinfo().to_be_bytes());
            bytes.extend_from_slice(&address.scope_id().to_be_bytes());
        }
    }
    if let Some(key) = key {
        bytes.extend_from_slice(key.expose_bytes());
        bytes.extend_from_slice(&public_key(key)?);
    }
    bytes.push(u8::from(public.is_some()));
    if let Some(public) = public {
        encode_public(&mut bytes, public);
    }
    Ok(bytes)
}
fn public_key(key: &PrivateKeyMaterial) -> Result<[u8; DeviceKey::LENGTH], PrivateStateError> {
    // Existing crypto owns derivation; the temporary native seed copy is erased.
    NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new(*key.expose_bytes())))
        .map(|identity| identity.public_key())
        .map_err(|_| PrivateStateError::Corrupt)
}
struct Input<'a>(&'a [u8]);
impl Input<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], PrivateStateError> {
        let (value, tail) = self
            .0
            .split_first_chunk::<N>()
            .ok_or(PrivateStateError::Corrupt)?;
        self.0 = tail;
        Ok(*value)
    }
    fn identifier(&mut self) -> Result<String, PrivateStateError> {
        let length = usize::from(u16::from_be_bytes(self.take()?));
        if length > MAX_IDENTIFIER_BYTES || self.0.len() < length {
            return Err(PrivateStateError::Corrupt);
        }
        let (value, tail) = self.0.split_at(length);
        self.0 = tail;
        String::from_utf8(value.to_vec()).map_err(|_| PrivateStateError::Corrupt)
    }
}
fn decode(bytes: &[u8]) -> Result<OwnerPhonePrivateOriginal, PrivateStateError> {
    let corrupt = |_| PrivateStateError::Corrupt;
    let mut input = Input(bytes);
    let prepared = match &input.take::<4>()? {
        value if value == RESERVED => false,
        value if value == PREPARED => true,
        _ => return Err(PrivateStateError::Corrupt),
    };
    let id = OwnerPhoneRequestId::new(input.take()?).map_err(|_| PrivateStateError::Corrupt)?;
    let owner = PrincipalId::new(input.identifier()?).map_err(corrupt)?;
    let membership = MembershipId::new(input.identifier()?).map_err(corrupt)?;
    let audience = AudienceId::new(input.identifier()?).map_err(corrupt)?;
    let credential = CredentialId::new(input.identifier()?).map_err(corrupt)?;
    let organization = OrganizationId::new(input.identifier()?).map_err(corrupt)?;
    let resource = ResourceId::new(input.identifier()?).map_err(corrupt)?;
    let source = OwnerPhoneSource::new(
        owner,
        membership,
        audience,
        credential,
        Resource::new(organization, resource),
    )
    .map_err(|_| PrivateStateError::Corrupt)?;
    let pin = input.take()?;
    if relay_key(&pin).is_none() {
        return Err(PrivateStateError::Corrupt);
    }
    let endpoint = match input.take::<1>()?[0] {
        4 => SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::from(input.take::<4>()?),
            u16::from_be_bytes(input.take()?),
        )),
        6 => SocketAddr::V6(SocketAddrV6::new(
            Ipv6Addr::from(input.take::<16>()?),
            u16::from_be_bytes(input.take()?),
            u32::from_be_bytes(input.take()?),
            u32::from_be_bytes(input.take()?),
        )),
        _ => return Err(PrivateStateError::Corrupt),
    };
    let context =
        OwnerPhoneContext::new(source, pin, endpoint).map_err(|_| PrivateStateError::Corrupt)?;
    let reservation = OwnerPhoneReservation::new(id, context);
    let key = if prepared {
        let key = PrivateKeyMaterial::new(Zeroizing::new(input.take()?));
        let published_key = input.take::<{ DeviceKey::LENGTH }>()?;
        if public_key(&key)? != published_key {
            return Err(PrivateStateError::Corrupt);
        }
        Some(key)
    } else {
        None
    };
    let public = match input.take::<1>()?[0] {
        0 => None,
        1 if prepared => Some(decode_public(
            &input.take::<PUBLIC_INTENT_BYTES>()?,
            ConsentClass::OwnerPhoneReadWrite,
        )?),
        _ => return Err(PrivateStateError::Corrupt),
    };
    if !input.0.is_empty() {
        return Err(PrivateStateError::Corrupt);
    }
    Ok(match (key, public) {
        (Some(key), Some(public)) => OwnerPhonePrivateOriginal::bound(reservation, key, public),
        (Some(key), None) => OwnerPhonePrivateOriginal::prepared(reservation, key),
        (None, None) => OwnerPhonePrivateOriginal::reserved(reservation),
        (None, Some(_)) => return Err(PrivateStateError::Corrupt),
    })
}

mod adoption;

#[cfg(test)]
#[path = "../../../../tests/adapters/private_state/owner_phone.rs"]
mod tests;
