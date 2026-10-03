use super::{PairingContext, PairingCryptoError};
use crate::{
    application::pairing::{DeviceConnectionProof, PrivateKeyMaterial},
    domain::pairing::{DeviceKey, PublicIntent},
};
use opaque_ke::rand::{CryptoRng, RngCore};
use ring::signature::{Ed25519KeyPair, KeyPair};
use rustls::{
    client::{
        danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
        AlwaysResolvesClientRawPublicKeys, Resumption,
    },
    crypto::{
        ring::{default_provider, sign::any_supported_type},
        verify_tls13_signature_with_raw_key,
    },
    pki_types::{
        CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, SubjectPublicKeyInfoDer,
        UnixTime,
    },
    server::{
        danger::{ClientCertVerified, ClientCertVerifier},
        AlwaysResolvesServerRawPublicKeys, NoServerSessionStorage,
    },
    sign::CertifiedKey,
    ClientConfig, ClientConnection, Connection, DigitallySignedStruct, DistinguishedName, Error,
    ServerConfig, ServerConnection, SignatureScheme, StreamOwned,
};
use std::{
    fmt,
    io::{self, Read, Write},
    sync::Arc,
};
use zeroize::{Zeroize, Zeroizing};

const SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];
const PKCS8_PREFIX: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];
const ALPN: &[u8] = b"nessa-device-v1";
const TLS_BUFFER_BYTES: usize = 64 * 1024;
const TLS_HANDSHAKE_BYTES: usize = 4096;
fn parse_spki(bytes: &[u8]) -> Result<[u8; 44], Error> {
    if bytes.len() != 44 || bytes[..12] != SPKI_PREFIX {
        return Err(Error::General("invalid native public key".into()));
    }
    let mut result = [0; 44];
    result.copy_from_slice(bytes);
    Ok(result)
}
/// Permanent Ed25519 identity; Nessa-owned seed and PKCS8 copies erase on drop.
/// The signing library does not expose an erasure guarantee for its internal copies.
pub struct NativeIdentity {
    seed: PrivateKeyMaterial,
    spki: [u8; 44],
}
impl NativeIdentity {
    /// Generate a permanent key from injected cryptographic entropy.
    pub fn generate(rng: &mut (impl RngCore + CryptoRng)) -> Result<Self, PairingCryptoError> {
        let mut seed = Zeroizing::new([0; 32]);
        rng.try_fill_bytes(seed.as_mut())
            .map_err(|_| PairingCryptoError::Unavailable)?;
        Self::restore(PrivateKeyMaterial::new(seed))
    }
    /// Restore exactly one durable private seed from the private local storage owner.
    pub fn restore(seed: PrivateKeyMaterial) -> Result<Self, PairingCryptoError> {
        let pair = Ed25519KeyPair::from_seed_unchecked(seed.expose_bytes())
            .map_err(|_| PairingCryptoError::Unavailable)?;
        let mut spki = [0; 44];
        spki[..12].copy_from_slice(&SPKI_PREFIX);
        spki[12..].copy_from_slice(pair.public_key().as_ref());
        Ok(Self { seed, spki })
    }
    /// Borrow the private seed only for atomic private pending-state persistence.
    pub fn key_material(&self) -> &PrivateKeyMaterial {
        &self.seed
    }
    /// Exact bounded Ed25519 SPKI pin.
    pub fn public_spki(&self) -> [u8; 44] {
        self.spki
    }
    fn certified_key(&self) -> Result<Arc<CertifiedKey>, PairingCryptoError> {
        let mut bytes = Zeroizing::new(Vec::with_capacity(48));
        bytes.extend_from_slice(&PKCS8_PREFIX);
        bytes.extend_from_slice(self.seed.expose_bytes());
        let mut key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(bytes.to_vec()));
        let signing = any_supported_type(&key);
        key.zeroize();
        let signing = signing.map_err(|_| PairingCryptoError::Unavailable)?;
        Ok(Arc::new(CertifiedKey::new(
            vec![CertificateDer::from(self.spki.to_vec())],
            signing,
        )))
    }
}
impl fmt::Debug for NativeIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NativeIdentity([REDACTED])")
    }
}
/// Gateway trust for this native connection. Bootstrap permits enrollment messages only.
#[derive(Clone, Copy, Debug)]
pub enum GatewayTrust {
    /// Initial manual-code connection; OPAQUE must authenticate the observed key.
    ManualBootstrap,
    /// Previously authenticated exact gateway SPKI; all future connections pin it.
    Pinned([u8; 44]),
}
#[derive(Debug)]
struct RawVerifier {
    pin: Option<[u8; 44]>,
}
impl RawVerifier {
    fn verify_key(
        &self,
        key: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
    ) -> Result<(), Error> {
        let key = parse_spki(key.as_ref())?;
        if !intermediates.is_empty() || self.pin.is_some_and(|pin| pin != key) {
            return Err(Error::General("native key mismatch".into()));
        }
        Ok(())
    }
    fn signature(
        &self,
        message: &[u8],
        key: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        parse_spki(key.as_ref())?;
        verify_tls13_signature_with_raw_key(
            message,
            &SubjectPublicKeyInfoDer::from(key.as_ref()),
            signature,
            &default_provider().signature_verification_algorithms,
        )
    }
}
impl ServerCertVerifier for RawVerifier {
    fn verify_server_cert(
        &self,
        key: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        self.verify_key(key, intermediates)?;
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::General("TLS 1.2 is unavailable".into()))
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        key: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.signature(message, key, signature)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}
impl ClientCertVerifier for RawVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        key: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _: UnixTime,
    ) -> Result<ClientCertVerified, Error> {
        self.verify_key(key, intermediates)?;
        Ok(ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::General("TLS 1.2 is unavailable".into()))
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        key: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.signature(message, key, signature)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}
/// Fully handshaken mutually signed native TLS transport; raw connection fields stay private.
/// Composition supplies a stream with its deadlines and physical-worker lifetime.
pub struct NativeTransport<S: Read + Write> {
    tls: NativeStream<S>,
    gateway: [u8; 44],
    device: [u8; 44],
    exporter: [u8; 32],
    device_proof: DeviceConnectionProof,
}
impl<S: Read + Write> NativeTransport<S> {
    /// Complete TLS 1.3 with mandatory raw device-key proof before returning a transport.
    pub fn accept(stream: S, identity: &NativeIdentity) -> Result<Self, PairingCryptoError> {
        let mut config = ServerConfig::builder_with_provider(Arc::new(default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|_| PairingCryptoError::Unavailable)?
            .with_client_cert_verifier(Arc::new(RawVerifier { pin: None }))
            .with_cert_resolver(Arc::new(AlwaysResolvesServerRawPublicKeys::new(
                identity.certified_key()?,
            )));
        config.alpn_protocols = vec![ALPN.to_vec()];
        config.max_early_data_size = 0;
        config.session_storage = Arc::new(NoServerSessionStorage {});
        config.send_tls13_tickets = 0;
        let connection =
            ServerConnection::new(Arc::new(config)).map_err(|_| PairingCryptoError::Unavailable)?;
        Self::complete(Connection::Server(connection), stream, identity.spki, true)
    }
    /// Complete a native TLS connection with real signature verification and optional strict pin.
    /// Manual bootstrap authenticates no product authority until the matching PAKE succeeds.
    pub fn connect(
        stream: S,
        identity: &NativeIdentity,
        trust: GatewayTrust,
    ) -> Result<Self, PairingCryptoError> {
        let pin = match trust {
            GatewayTrust::ManualBootstrap => None,
            GatewayTrust::Pinned(pin) => {
                parse_spki(&pin).map_err(|_| PairingCryptoError::InvalidContext)?;
                Some(pin)
            }
        };
        let mut config = ClientConfig::builder_with_provider(Arc::new(default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|_| PairingCryptoError::Unavailable)?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(RawVerifier { pin }))
            .with_client_cert_resolver(Arc::new(AlwaysResolvesClientRawPublicKeys::new(
                identity.certified_key()?,
            )));
        config.alpn_protocols = vec![ALPN.to_vec()];
        config.resumption = Resumption::disabled();
        config.enable_early_data = false;
        let name =
            ServerName::try_from("nessa.invalid").map_err(|_| PairingCryptoError::Unavailable)?;
        let connection = ClientConnection::new(Arc::new(config), name)
            .map_err(|_| PairingCryptoError::Unavailable)?;
        Self::complete(Connection::Client(connection), stream, identity.spki, false)
    }
    fn complete(
        mut connection: Connection,
        mut stream: S,
        local: [u8; 44],
        server: bool,
    ) -> Result<Self, PairingCryptoError> {
        connection.set_buffer_limit(Some(TLS_BUFFER_BYTES));
        let mut budget = HandshakeBudget {
            stream: &mut stream,
            read: 0,
            written: 0,
        };
        while connection.is_handshaking() {
            connection
                .complete_io(&mut budget)
                .map_err(|_| PairingCryptoError::InvalidProof)?;
        }
        if connection.alpn_protocol() != Some(ALPN) {
            return Err(PairingCryptoError::InvalidContext);
        }
        let certificates = connection
            .peer_certificates()
            .ok_or(PairingCryptoError::InvalidProof)?;
        let peer = certificates
            .first()
            .ok_or(PairingCryptoError::InvalidProof)?;
        let peer = parse_spki(peer.as_ref()).map_err(|_| PairingCryptoError::InvalidProof)?;
        let exporter = connection
            .export_keying_material([0; 32], b"EXPORTER-Channel-Binding", Some(&[]))
            .map_err(|_| PairingCryptoError::InvalidProof)?;
        let (gateway, device) = if server { (local, peer) } else { (peer, local) };
        let mut proved_key = [0; 32];
        proved_key.copy_from_slice(&device[12..]);
        let device_proof = DeviceConnectionProof::from_tls(DeviceKey::new(proved_key));
        Ok(Self {
            device_proof,
            tls: match connection {
                Connection::Client(connection) => {
                    NativeStream::Client(StreamOwned::new(connection, stream))
                }
                Connection::Server(connection) => {
                    NativeStream::Server(StreamOwned::new(connection, stream))
                }
            },
            gateway,
            device,
            exporter,
        })
    }
    /// Borrow the originally injected physical stream for its composition-owned
    /// readiness/deadline controls. This does not establish application authority.
    pub fn stream_mut(&mut self) -> &mut S {
        match &mut self.tls {
            NativeStream::Client(stream) => &mut stream.sock,
            NativeStream::Server(stream) => &mut stream.sock,
        }
    }
    /// Actual TLS output state used to register nonblocking socket readiness.
    pub fn wants_write(&self) -> bool {
        match &self.tls {
            NativeStream::Client(stream) => stream.conn.wants_write(),
            NativeStream::Server(stream) => stream.conn.wants_write(),
        }
    }
    /// Build canonical PAKE context from locally derived complete-handshake evidence.
    pub fn pairing_context(
        &self,
        public: PublicIntent,
    ) -> Result<PairingContext, PairingCryptoError> {
        PairingContext::from_transport(public, self.gateway, self.device, self.exporter)
    }
    /// Possession evidence from this complete signed native channel, not a request DTO.
    pub fn device_proof(&self) -> &DeviceConnectionProof {
        &self.device_proof
    }

    /// Exact observed gateway key. Bootstrap callers persist it only after valid KE2.
    pub fn gateway_spki(&self) -> [u8; 44] {
        self.gateway
    }
}
// Complete-message framing belongs to the native server codec. These operations
// preserve the original live TLS/proof owner and expose decrypted stream I/O.
impl<S: Read + Write> Read for NativeTransport<S> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.tls.read(bytes)
    }
}
impl<S: Read + Write> Write for NativeTransport<S> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.tls.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.tls.flush()
    }
}
struct HandshakeBudget<'a, S> {
    stream: &'a mut S,
    read: usize,
    written: usize,
}
impl<S: Read> Read for HandshakeBudget<'_, S> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let remaining = TLS_HANDSHAKE_BYTES
            .checked_sub(self.read)
            .filter(|remaining| *remaining > 0)
            .ok_or_else(|| io::Error::other("native handshake ingress limit"))?;
        let length = bytes.len().min(remaining);
        let count = self.stream.read(&mut bytes[..length])?;
        self.read += count;
        Ok(count)
    }
}
impl<S: Write> Write for HandshakeBudget<'_, S> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let remaining = TLS_HANDSHAKE_BYTES
            .checked_sub(self.written)
            .filter(|remaining| *remaining > 0)
            .ok_or_else(|| io::Error::other("native handshake egress limit"))?;
        let count = self.stream.write(&bytes[..bytes.len().min(remaining)])?;
        self.written += count;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

enum NativeStream<S: Read + Write> {
    Client(StreamOwned<ClientConnection, S>),
    Server(StreamOwned<ServerConnection, S>),
}
impl<S: Read + Write> Read for NativeStream<S> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Client(stream) => stream.read(bytes),
            Self::Server(stream) => stream.read(bytes),
        }
    }
}
impl<S: Read + Write> Write for NativeStream<S> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            Self::Client(stream) => stream.write(bytes),
            Self::Server(stream) => stream.write(bytes),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Client(stream) => stream.flush(),
            Self::Server(stream) => stream.flush(),
        }
    }
}

#[cfg(test)]
#[path = "tls/tests.rs"]
mod tests;
