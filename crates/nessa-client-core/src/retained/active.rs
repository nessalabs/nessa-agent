//! Device read selection binds fresh pinned status to its exact issued record.
use super::{PinnedStatus, ReaderAccess, ReaderRole};
use crate::pairing::NativeClientError;
use nessa_auth::{
    adapters::pairing::NativeIdentity,
    application::pairing::{ClientPendingStore, DeviceCredential, PrivateStateError},
};
use nessa_protocol::pairing::wire::{NativePairingStatus, NativeWireError};
use std::net::SocketAddr;

/// Owned device key binding selected from one fresh pinned status and its
/// immutable issued record. It does not authorize a request; the gateway does.
pub struct RetainedDeviceReader {
    identity: NativeIdentity,
    pin: [u8; 44],
    credential: String,
    receiver: String,
    access_epoch: u64,
}

#[cfg(test)]
#[path = "../../tests/retained/active.rs"]
mod tests;
impl RetainedDeviceReader {
    /// Select the current issued binding only for an Active pinned status. This
    /// keeps record loading after status publication and before cache/product I/O.
    pub fn from_current(
        status: &NativePairingStatus,
        state: &dyn ClientPendingStore,
    ) -> Result<Option<Self>, NativeClientError> {
        match PinnedStatus::from_native(status) {
            Some(PinnedStatus::Active { .. }) => {}
            Some(PinnedStatus::Terminal | PinnedStatus::NotActive) => return Ok(None),
            None => return Err(NativeClientError::Wire(NativeWireError::Invalid)),
        }
        let issued = state
            .load_credential()
            .map_err(NativeClientError::Storage)?
            .ok_or(NativeClientError::NoPending)?;
        Self::from_issued(status, issued)
    }
    /// Refuse contradictory intent/credential/receiver before opening the
    /// cache or product channel. Non-active status selects no reader.
    pub fn from_issued(
        status: &NativePairingStatus,
        issued: DeviceCredential,
    ) -> Result<Option<Self>, NativeClientError> {
        let decision = PinnedStatus::from_native(status)
            .ok_or(NativeClientError::Wire(NativeWireError::Invalid))?;
        let PinnedStatus::Active { access_epoch, .. } = decision else {
            return Ok(None);
        };
        let NativePairingStatus::Active {
            consent,
            credential,
            receiver,
            ..
        } = status
        else {
            unreachable!("active classification has an active source variant")
        };
        if consent.public() != issued.intent()
            || credential != issued.credential()
            || receiver != issued.receiver()
        {
            return Err(NativeClientError::Storage(PrivateStateError::Conflict));
        }
        let credential = credential.as_str().to_owned();
        let receiver = receiver.as_str().to_owned();
        let (key, pin, _) = issued.into_enrollment().into_parts();
        let identity = NativeIdentity::restore(key).map_err(NativeClientError::Crypto)?;
        Ok(Some(Self {
            identity,
            pin,
            credential,
            receiver,
            access_epoch,
        }))
    }
    /// The receiver this binding was selected for.
    pub fn receiver(&self) -> &str {
        &self.receiver
    }
    /// Select the existing owner catalogue using the retained exact binding.
    pub fn access<'a>(&'a self, address: SocketAddr, client_id: &'a str) -> ReaderAccess<'a> {
        ReaderAccess {
            role: ReaderRole::DeviceOwner,
            address,
            relay: None,
            identity: &self.identity,
            pin: self.pin,
            credential: &self.credential,
            receiver: &self.receiver,
            access_epoch: self.access_epoch,
            client_id,
        }
    }
}
