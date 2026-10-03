use super::PairingCryptoError;
use crate::domain::pairing::PublicIntent;

pub(crate) const DOMAIN: &[u8] = b"nessa-device-pairing-v1";
const SUITE: &[u8] = b"opaque-ke-4.0.1/ristretto255/tripledh/sha512/argon2id-19-m65536-t3-p4-o64";
/// Canonical bounded context passed directly into OPAQUE's native transcript.
/// The transport owner constructs it from its own verified TLS key and exporter.
pub struct PairingContext {
    pub(crate) public: PublicIntent,
    pub(crate) bytes: Vec<u8>,
    pub(crate) gateway: [u8; 44],
    pub(crate) device: [u8; 44],
    pub(crate) client_identity: Vec<u8>,
    pub(crate) server_identity: Vec<u8>,
}
impl PairingContext {
    pub(crate) fn from_transport(
        public: PublicIntent,
        gateway: [u8; 44],
        device: [u8; 44],
        exporter: [u8; 32],
    ) -> Result<Self, PairingCryptoError> {
        let mut bytes = Vec::with_capacity(512);
        for part in [
            DOMAIN,
            SUITE,
            b"manual".as_slice(),
            &gateway,
            &device,
            public.invitation().bytes(),
            public.attempt().bytes(),
            &public.expiry_ms().to_be_bytes(),
            public.consent().bytes(),
            &public.generation().to_be_bytes(),
            public.class().as_bytes(),
            &exporter,
        ] {
            let length =
                u16::try_from(part.len()).map_err(|_| PairingCryptoError::InvalidContext)?;
            bytes.extend_from_slice(&length.to_be_bytes());
            bytes.extend_from_slice(part);
        }
        let mut client_identity = DOMAIN.to_vec();
        client_identity.extend_from_slice(public.invitation().bytes());
        let mut server_identity = DOMAIN.to_vec();
        server_identity.extend_from_slice(&gateway);
        Ok(Self {
            public,
            bytes,
            gateway,
            device,
            client_identity,
            server_identity,
        })
    }
    /// Public transcript metadata, authenticated after successful client finish.
    pub fn public(&self) -> PublicIntent {
        self.public
    }
    /// Gateway pin authenticated after valid KE2; persist it in the pre-KE3 pending commit.
    pub fn gateway_spki(&self) -> [u8; 44] {
        self.gateway
    }
    /// Exact permanent device key bound into this transcript.
    pub fn device_spki(&self) -> [u8; 44] {
        self.device
    }
    /// Borrow the canonical bytes for protocol vectors; this exposes no invitation secret.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}
