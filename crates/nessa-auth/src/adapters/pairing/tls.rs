use super::{PairingContext, PairingCryptoError};
use crate::{
    application::pairing::{DeviceConnectionProof, PrivateKeyMaterial},
    domain::pairing::{DeviceKey, PublicIntent},
};
use opaque_ke::rand::{CryptoRng, RngCore};
use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519};
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
// Callers supply a locally constructed or already authenticated native SPKI.
// `local_device_key_matches_independent_ed25519_public_bytes` checks the extraction.
pub(super) fn device_key(spki: &[u8; 44]) -> DeviceKey {
    let mut key = [0; DeviceKey::LENGTH];
    key.copy_from_slice(&spki[SPKI_PREFIX.len()..]);
    DeviceKey::new(key)
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
    /// Local public key in the pairing owner's representation; not possession proof.
    pub fn device_key(&self) -> DeviceKey {
        device_key(&self.spki)
    }
    /// The raw Ed25519 public key: the id a relay registers this identity
    /// under (`docs/design/auth/relay.md`).
    pub fn public_key(&self) -> [u8; 32] {
        *self.device_key().bytes()
    }
    /// Prove this key to a relay: an Ed25519 signature over the relay proof
    /// message for `challenge`, the relay's fresh nonce. The message starts
    /// with [`RELAY_PROOF_CONTEXT`], never with the 64 spaces a TLS 1.3
    /// handshake signature starts with, so a relay cannot use its challenge
    /// to get a signature it could present in a handshake.
    pub fn sign_relay_challenge(
        &self,
        challenge: &[u8; 32],
    ) -> Result<[u8; 64], PairingCryptoError> {
        let pair = Ed25519KeyPair::from_seed_unchecked(self.seed.expose_bytes())
            .map_err(|_| PairingCryptoError::Unavailable)?;
        let signature = pair.sign(&relay_proof_message(&self.public_key(), challenge));
        let mut bytes = [0; 64];
        bytes.copy_from_slice(signature.as_ref());
        Ok(bytes)
    }
    /// Sign a tunnel origin this gateway is about to publish. The message
    /// starts with [`TUNNEL_ORIGIN_CONTEXT`], never with the 64 spaces a TLS
    /// 1.3 handshake signature starts with. A phone checks the signature
    /// against the key it pinned before it sends its tunnel secret.
    pub fn sign_tunnel_origin(&self, origin: &[u8]) -> Result<[u8; 64], PairingCryptoError> {
        let pair = Ed25519KeyPair::from_seed_unchecked(self.seed.expose_bytes())
            .map_err(|_| PairingCryptoError::Unavailable)?;
        let signature = pair.sign(&tunnel_origin_message(&self.public_key(), origin));
        let mut bytes = [0; 64];
        bytes.copy_from_slice(signature.as_ref());
        Ok(bytes)
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
/// What every relay proof message starts with.
pub const RELAY_PROOF_CONTEXT: &[u8] = b"nessa-relay-serve-v1\0";
/// What every published-tunnel-origin signature starts with.
pub const TUNNEL_ORIGIN_CONTEXT: &[u8] = b"nessa-tunnel-origin-v1\0";
/// The context, the key, then the challenge: the one layout a relay proof
/// signs.
fn relay_proof_message(key: &[u8; 32], challenge: &[u8; 32]) -> Vec<u8> {
    let mut message = Vec::with_capacity(RELAY_PROOF_CONTEXT.len() + 64);
    message.extend_from_slice(RELAY_PROOF_CONTEXT);
    message.extend_from_slice(key);
    message.extend_from_slice(challenge);
    message
}
/// Whether `signature` is `key`'s relay proof for `challenge`, as
/// [`NativeIdentity::sign_relay_challenge`] makes it.
pub fn verify_relay_proof(key: &[u8; 32], challenge: &[u8; 32], signature: &[u8; 64]) -> bool {
    UnparsedPublicKey::new(&ED25519, key)
        .verify(&relay_proof_message(key, challenge), signature)
        .is_ok()
}
fn tunnel_origin_message(key: &[u8; 32], origin: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(TUNNEL_ORIGIN_CONTEXT.len() + 32 + origin.len());
    message.extend_from_slice(TUNNEL_ORIGIN_CONTEXT);
    message.extend_from_slice(key);
    message.extend_from_slice(origin);
    message
}
/// Whether `signature` is `key`'s signature over `origin`, as
/// [`NativeIdentity::sign_tunnel_origin`] makes it.
pub fn verify_tunnel_origin(key: &[u8; 32], origin: &[u8], signature: &[u8; 64]) -> bool {
    UnparsedPublicKey::new(&ED25519, key)
        .verify(&tunnel_origin_message(key, origin), signature)
        .is_ok()
}
/// The raw key inside a pinned gateway SPKI: the id the gateway is
/// registered under at a relay. `None` when `pin` is not an Ed25519 SPKI.
pub fn relay_key(pin: &[u8; 44]) -> Option<[u8; 32]> {
    let spki = parse_spki(pin).ok()?;
    Some(*device_key(&spki).bytes())
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
        Self::connect_observed(stream, identity, trust, |_| {})
    }
    /// Observe the canonical typed failure before dropping the owned physical
    /// stream, so its owner can retain the first cause while cleanup drains.
    /// Successful connections emit no failure. The synchronous observer must
    /// return promptly without panicking; it receives no credential or key.
    pub fn connect_observed(
        stream: S,
        identity: &NativeIdentity,
        trust: GatewayTrust,
        mut observer: impl FnMut(PairingCryptoError),
    ) -> Result<Self, PairingCryptoError> {
        let configured = (|| {
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
            let name = ServerName::try_from("nessa.invalid")
                .map_err(|_| PairingCryptoError::Unavailable)?;
            let connection = ClientConnection::new(Arc::new(config), name)
                .map_err(|_| PairingCryptoError::Unavailable)?;
            Ok(Connection::Client(connection))
        })();
        match configured {
            Ok(connection) => {
                Self::complete_observed(connection, stream, identity.spki, false, observer)
            }
            Err(error) => {
                observer(error);
                Err(error)
            }
        }
    }
    fn complete(
        connection: Connection,
        stream: S,
        local: [u8; 44],
        server: bool,
    ) -> Result<Self, PairingCryptoError> {
        Self::complete_observed(connection, stream, local, server, |_| {})
    }
    fn complete_observed(
        mut connection: Connection,
        mut stream: S,
        local: [u8; 44],
        server: bool,
        mut observer: impl FnMut(PairingCryptoError),
    ) -> Result<Self, PairingCryptoError> {
        let validated = (|| {
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
            let device_proof = DeviceConnectionProof::from_tls(device_key(&device));
            Ok((gateway, device, exporter, device_proof))
        })();
        let (gateway, device, exporter, device_proof) = match validated {
            Ok(facts) => facts,
            Err(error) => {
                observer(error);
                return Err(error);
            }
        };
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
    /// Request TLS traffic-key rotation through rustls and flush its control record.
    /// Permanent identity, possession proof and enrollment authority do not change.
    /// The supplied stream retains responsibility for I/O deadlines and cancellation.
    pub fn request_key_update(&mut self) -> io::Result<()> {
        self.tls.request_key_update()?;
        self.tls.flush()
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
impl<S: Read + Write> NativeStream<S> {
    fn request_key_update(&mut self) -> io::Result<()> {
        let result = match self {
            Self::Client(stream) => stream.conn.refresh_traffic_keys(),
            Self::Server(stream) => stream.conn.refresh_traffic_keys(),
        };
        result.map_err(io::Error::other)
    }
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
