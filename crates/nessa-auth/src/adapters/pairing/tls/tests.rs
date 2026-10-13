use super::*;
use ring::signature::{Ed25519KeyPair, KeyPair};

#[cfg(unix)]
use ring::rand::SystemRandom;
#[cfg(unix)]
use rustls::{
    sign::{Signer, SigningKey},
    ProtocolVersion, SignatureAlgorithm,
};
use std::{
    io::Cursor,
    sync::atomic::{AtomicUsize, Ordering},
};
#[cfg(unix)]
use std::{os::unix::net::UnixStream, thread, time::Duration};

#[test]
fn local_device_key_matches_independent_ed25519_public_bytes() {
    for seed in [[0; 32], [7; 32], [255; 32]] {
        let expected = Ed25519KeyPair::from_seed_unchecked(&seed).unwrap();
        let identity =
            NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new(seed))).unwrap();
        assert_eq!(
            identity.device_key().bytes(),
            expected.public_key().as_ref()
        );
        assert_eq!(identity.device_key(), device_key(&identity.public_spki()));
        assert_eq!(
            identity.public_key().as_slice(),
            expected.public_key().as_ref()
        );
    }
}

#[cfg(unix)]
#[derive(Debug)]
struct PeerSigningKey {
    original: Arc<dyn SigningKey>,
    corrupt: bool,
    signatures: Arc<AtomicUsize>,
}
#[cfg(unix)]
impl SigningKey for PeerSigningKey {
    fn choose_scheme(&self, offered: &[SignatureScheme]) -> Option<Box<dyn Signer>> {
        self.original.choose_scheme(offered).map(|original| {
            Box::new(PeerSigner {
                original,
                corrupt: self.corrupt,
                signatures: self.signatures.clone(),
            }) as Box<dyn Signer>
        })
    }
    fn public_key(&self) -> Option<SubjectPublicKeyInfoDer<'_>> {
        self.original.public_key()
    }
    fn algorithm(&self) -> SignatureAlgorithm {
        self.original.algorithm()
    }
}
#[cfg(unix)]
#[derive(Debug)]
struct PeerSigner {
    original: Box<dyn Signer>,
    corrupt: bool,
    signatures: Arc<AtomicUsize>,
}
#[cfg(unix)]
impl Signer for PeerSigner {
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, Error> {
        let mut signature = self.original.sign(message)?;
        self.signatures.fetch_add(1, Ordering::SeqCst);
        if self.corrupt {
            signature[0] ^= 1;
        }
        Ok(signature)
    }
    fn scheme(&self) -> SignatureScheme {
        self.original.scheme()
    }
}
#[cfg(unix)]
#[derive(Debug)]
struct PeerServerVerifier {
    expected: [u8; 44],
}
#[cfg(unix)]
impl ServerCertVerifier for PeerServerVerifier {
    fn verify_server_cert(
        &self,
        key: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        if key.as_ref() != self.expected || !intermediates.is_empty() {
            return Err(Error::General("unexpected fixture server key".into()));
        }
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::General("fixture requires TLS 1.3".into()))
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        key: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature_with_raw_key(
            message,
            &SubjectPublicKeyInfoDer::from(key.as_ref()),
            signature,
            &default_provider().signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}
#[cfg(unix)]
fn native_peer(
    gateway: [u8; 44],
    corrupt: bool,
    alpn: bool,
    signatures: Arc<AtomicUsize>,
) -> (ClientConnection, [u8; 44]) {
    // This external peer uses ring's public PKCS8 producer and rustls's public
    // signing provider. It does not use Nessa's private identity/verifier setup.
    let document = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
    let pair = Ed25519KeyPair::from_pkcs8(document.as_ref()).unwrap();
    let mut spki = [0; 44];
    spki[..12].copy_from_slice(&[
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ]);
    spki[12..].copy_from_slice(pair.public_key().as_ref());
    let private = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(document.as_ref().to_vec()));
    let original = any_supported_type(&private).unwrap();
    let key = Arc::new(CertifiedKey::new(
        vec![CertificateDer::from(spki.to_vec())],
        Arc::new(PeerSigningKey {
            original,
            corrupt,
            signatures,
        }),
    ));
    let mut config = ClientConfig::builder_with_provider(Arc::new(default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PeerServerVerifier { expected: gateway }))
        .with_client_cert_resolver(Arc::new(AlwaysResolvesClientRawPublicKeys::new(key)));
    if alpn {
        config.alpn_protocols = vec![ALPN.to_vec()];
    }
    config.resumption = Resumption::disabled();
    config.enable_early_data = false;
    let connection = ClientConnection::new(
        Arc::new(config),
        ServerName::try_from("nessa.invalid").unwrap(),
    )
    .unwrap();
    (connection, spki)
}
#[cfg(unix)]
fn peer_streams() -> (UnixStream, UnixStream) {
    let (gateway, device) = UnixStream::pair().unwrap();
    for stream in [&gateway, &device] {
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(30)))
            .unwrap();
    }
    (gateway, device)
}
#[cfg(unix)]
fn complete_peer(
    mut connection: ClientConnection,
    mut stream: UnixStream,
) -> io::Result<(ClientConnection, UnixStream)> {
    while connection.is_handshaking() {
        connection.complete_io(&mut stream)?;
    }
    Ok((connection, stream))
}

#[cfg(unix)]
#[test]
fn native_tls_refuses_corrupted_peer_signature_and_accepts_original() {
    for corrupt in [true, false] {
        let gateway =
            NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([10; 32]))).unwrap();
        let signatures = Arc::new(AtomicUsize::new(0));
        let (peer, device) = native_peer(gateway.public_spki(), corrupt, true, signatures.clone());
        let (a, b) = peer_streams();
        let server = thread::spawn(move || NativeTransport::accept(a, &gateway));
        let peer_result = complete_peer(peer, b);
        let server_result = server.join().unwrap();
        assert_eq!(signatures.load(Ordering::SeqCst), 1);
        if corrupt {
            assert!(matches!(
                server_result,
                Err(PairingCryptoError::InvalidProof)
            ));
        } else {
            let (peer, _stream) = peer_result.unwrap();
            assert_eq!(peer.protocol_version(), Some(ProtocolVersion::TLSv1_3));
            assert_eq!(peer.alpn_protocol(), Some(ALPN));
            let transport = server_result.unwrap();
            assert_eq!(transport.device_proof().key().bytes(), &device[12..]);
        }
    }
}

#[cfg(unix)]
#[test]
fn native_tls_refuses_completed_peer_without_alpn_and_accepts_original() {
    for alpn in [false, true] {
        let gateway =
            NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([10; 32]))).unwrap();
        let signatures = Arc::new(AtomicUsize::new(0));
        let (peer, device) = native_peer(gateway.public_spki(), false, alpn, signatures.clone());
        let (a, b) = peer_streams();
        let server = thread::spawn(move || NativeTransport::accept(a, &gateway));
        let peer_result = complete_peer(peer, b);
        let server_result = server.join().unwrap();
        let (peer, _stream) = peer_result.unwrap();
        assert_eq!(signatures.load(Ordering::SeqCst), 1);
        assert_eq!(peer.protocol_version(), Some(ProtocolVersion::TLSv1_3));
        if alpn {
            assert_eq!(peer.alpn_protocol(), Some(ALPN));
            let transport = server_result.unwrap();
            assert_eq!(transport.device_proof().key().bytes(), &device[12..]);
        } else {
            assert_eq!(peer.alpn_protocol(), None);
            assert!(matches!(
                server_result,
                Err(PairingCryptoError::InvalidContext)
            ));
        }
    }
}

#[test]
fn public_native_accept_bounds_actual_peer_ingress() {
    struct Input {
        bytes: Cursor<Vec<u8>>,
        read: Arc<AtomicUsize>,
    }
    impl Read for Input {
        fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
            let count = self.bytes.read(target)?;
            self.read.fetch_add(count, Ordering::SeqCst);
            Ok(count)
        }
    }
    impl Write for Input {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    // A real TLS record header declares a body beyond the native handshake bound.
    let mut bytes = vec![22, 3, 3, 0x20, 0x00];
    bytes.resize(8197, 0);
    let read = Arc::new(AtomicUsize::new(0));
    let input = Input {
        bytes: Cursor::new(bytes),
        read: read.clone(),
    };
    let identity =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([9; 32]))).unwrap();
    assert!(matches!(
        NativeTransport::accept(input, &identity),
        Err(PairingCryptoError::InvalidProof)
    ));
    assert_eq!(read.load(Ordering::SeqCst), 4096);
}

fn relay_identity(seed: u8) -> NativeIdentity {
    NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([seed; 32]))).unwrap()
}

#[test]
fn a_relay_proof_verifies_only_for_its_key_and_its_challenge() {
    let identity = relay_identity(3);
    let other = relay_identity(4);
    let key = identity.public_key();
    assert_eq!(&identity.public_spki()[12..], &key);
    let challenge = [7; 32];
    let proof = identity.sign_relay_challenge(&challenge).unwrap();
    assert!(verify_relay_proof(&key, &challenge, &proof));
    // Another challenge, another key, or a flipped bit is not this proof.
    assert!(!verify_relay_proof(&key, &[8; 32], &proof));
    assert!(!verify_relay_proof(&other.public_key(), &challenge, &proof));
    let mut flipped = proof;
    flipped[0] ^= 1;
    assert!(!verify_relay_proof(&key, &challenge, &flipped));
    // Another key's proof of the same challenge does not stand for this key.
    let theirs = other.sign_relay_challenge(&challenge).unwrap();
    assert!(!verify_relay_proof(&key, &challenge, &theirs));
}

#[test]
fn a_relay_proof_signs_its_context_first_and_never_a_handshake_prefix() {
    let identity = relay_identity(5);
    let key = identity.public_key();
    let challenge = [9; 32];
    let proof = identity.sign_relay_challenge(&challenge).unwrap();
    let mut message = RELAY_PROOF_CONTEXT.to_vec();
    message.extend_from_slice(&key);
    message.extend_from_slice(&challenge);
    UnparsedPublicKey::new(&ED25519, key)
        .verify(&message, &proof)
        .unwrap();
    // TLS 1.3 signs 64 spaces, then its context; a relay proof never starts so.
    assert_ne!(RELAY_PROOF_CONTEXT[0], b' ');
}
