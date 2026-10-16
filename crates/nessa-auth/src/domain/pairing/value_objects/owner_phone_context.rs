//! Auth's exact discovery facts before allocating a permanent device key.
use super::{DeviceKey, OwnerPhoneRequest, OwnerPhoneRequestId, OwnerPhoneSource, PairingError};
use std::net::SocketAddr;

/// Immutable original destination and source; construction grants no authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerPhoneContext {
    source: OwnerPhoneSource,
    pin: [u8; 44],
    endpoint: SocketAddr,
}
impl OwnerPhoneContext {
    /// Use the same numeric destination owner as canonical request construction.
    /// The crypto adapter validates SPKI grammar before preparation effects.
    pub fn new(
        source: OwnerPhoneSource,
        pin: [u8; 44],
        endpoint: SocketAddr,
    ) -> Result<Self, PairingError> {
        OwnerPhoneRequest::validate_endpoint(endpoint)?;
        Ok(Self {
            source,
            pin,
            endpoint,
        })
    }
    /// Exact original public source, not a current authorization snapshot.
    pub fn source(&self) -> &OwnerPhoneSource {
        &self.source
    }
    /// Exact advertised public TLS pin.
    pub fn gateway_pin(&self) -> &[u8; 44] {
        &self.pin
    }
    /// Exact advertised numeric destination.
    pub fn endpoint(&self) -> SocketAddr {
        self.endpoint
    }
    /// Attach actual original correlation and a permanent public key.
    pub fn request(
        &self,
        id: OwnerPhoneRequestId,
        key: DeviceKey,
    ) -> Result<OwnerPhoneRequest, PairingError> {
        OwnerPhoneRequest::new(id, self.source.clone(), self.pin, self.endpoint, key)
    }
}
