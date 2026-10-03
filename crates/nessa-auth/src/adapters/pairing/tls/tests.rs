use super::*;
#[cfg(unix)]
use rustls::{
    sign::{Signer, SigningKey},
    ProtocolVersion, SignatureAlgorithm,
};
use std::io::Cursor;
#[cfg(unix)]
use std::{
    os::unix::net::UnixStream,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::Duration,
};

#[test]
fn cumulative_handshake_ingress_refuses_next_byte_before_underlying_read() {
    let mut input = Cursor::new(vec![7; TLS_HANDSHAKE_BYTES + 1]);
    let mut budget = HandshakeBudget {
        stream: &mut input,
        read: 0,
        written: 0,
    };
    let mut chunk = [0; 1024];
    for _ in 0..4 {
        budget.read_exact(&mut chunk).unwrap();
    }
    assert_eq!(budget.read, 4096);
    assert!(budget.read(&mut [0]).is_err());
    assert_eq!(input.position(), 4096);
}

#[test]
fn cumulative_handshake_egress_refuses_next_byte_before_underlying_write() {
    let mut output = Vec::new();
    let mut budget = HandshakeBudget {
        stream: &mut output,
        read: 0,
        written: 0,
    };
    for _ in 0..4 {
        budget.write_all(&[7; 1024]).unwrap();
    }
    assert_eq!(budget.written, 4096);
    assert!(budget.write(&[7]).is_err());
    assert_eq!(output.len(), 4096);
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
fn native_peer(
    identity: &NativeIdentity,
    corrupt: bool,
    alpn: bool,
    signatures: Arc<AtomicUsize>,
) -> ClientConnection {
    let original = identity.certified_key().unwrap();
    let key = Arc::new(CertifiedKey::new(
        original.cert.clone(),
        Arc::new(PeerSigningKey {
            original: original.key.clone(),
            corrupt,
            signatures,
        }),
    ));
    let mut config = ClientConfig::builder_with_provider(Arc::new(default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(RawVerifier { pin: None }))
        .with_client_cert_resolver(Arc::new(AlwaysResolvesClientRawPublicKeys::new(key)));
    if alpn {
        config.alpn_protocols = vec![ALPN.to_vec()];
    }
    config.resumption = Resumption::disabled();
    config.enable_early_data = false;
    ClientConnection::new(
        Arc::new(config),
        ServerName::try_from("nessa.invalid").unwrap(),
    )
    .unwrap()
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
        let device =
            NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([11; 32]))).unwrap();
        let signatures = Arc::new(AtomicUsize::new(0));
        let peer = native_peer(&device, corrupt, true, signatures.clone());
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
            assert_eq!(
                transport.device_proof().key().bytes(),
                &device.public_spki()[12..]
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn native_tls_refuses_completed_peer_without_alpn_and_accepts_original() {
    for alpn in [false, true] {
        let gateway =
            NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([10; 32]))).unwrap();
        let device =
            NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([11; 32]))).unwrap();
        let signatures = Arc::new(AtomicUsize::new(0));
        let peer = native_peer(&device, false, alpn, signatures.clone());
        let (a, b) = peer_streams();
        let server = thread::spawn(move || NativeTransport::accept(a, &gateway));
        let (peer, _stream) = complete_peer(peer, b).unwrap();
        let server_result = server.join().unwrap();
        assert_eq!(signatures.load(Ordering::SeqCst), 1);
        assert_eq!(peer.protocol_version(), Some(ProtocolVersion::TLSv1_3));
        if alpn {
            assert_eq!(peer.alpn_protocol(), Some(ALPN));
            let transport = server_result.unwrap();
            assert_eq!(
                transport.device_proof().key().bytes(),
                &device.public_spki()[12..]
            );
        } else {
            assert_eq!(peer.alpn_protocol(), None);
            assert!(matches!(
                server_result,
                Err(PairingCryptoError::InvalidContext)
            ));
        }
    }
}
