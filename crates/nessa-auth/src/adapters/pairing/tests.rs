pub(crate) mod io;
use super::*;
use crate::{
    application::pairing::PrivateKeyMaterial,
    domain::pairing::{AttemptId, ConsentIntentId, InvitationId, PublicIntent},
};
use io::native_channels;
#[cfg(unix)]
use io::{message_lengths, CryptoFixtureTransport};
use opaque_ke::rand::{CryptoRng, Error, RngCore};
use std::{
    net::TcpStream,
    num::NonZeroU32,
    sync::atomic::{AtomicBool, Ordering},
};
#[cfg(unix)]
use std::{os::unix::net::UnixStream, thread, time::Duration};
use zeroize::Zeroizing;

pub(crate) struct Entropy;
impl RngCore for Entropy {
    fn next_u32(&mut self) -> u32 {
        let mut bytes = [0; 4];
        self.fill_bytes(&mut bytes);
        u32::from_le_bytes(bytes)
    }
    fn next_u64(&mut self) -> u64 {
        let mut bytes = [0; 8];
        self.fill_bytes(&mut bytes);
        u64::from_le_bytes(bytes)
    }
    fn fill_bytes(&mut self, bytes: &mut [u8]) {
        self.try_fill_bytes(bytes).unwrap();
    }
    fn try_fill_bytes(&mut self, bytes: &mut [u8]) -> Result<(), Error> {
        getrandom::fill(bytes)
            .map_err(|_| Error::from(NonZeroU32::new(Error::CUSTOM_START).unwrap()))
    }
}
impl CryptoRng for Entropy {}
fn public() -> PublicIntent {
    PublicIntent::new(
        InvitationId::new([1; 16]),
        AttemptId::new([2; 16]),
        ConsentIntentId::new([3; 16]),
        1,
        600_000,
    )
    .unwrap()
}

#[cfg(unix)]
fn streams() -> (UnixStream, UnixStream) {
    let (a, b) = UnixStream::pair().unwrap();
    for stream in [&a, &b] {
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(30)))
            .unwrap();
    }
    (a, b)
}
#[cfg(unix)]
#[test]
fn native_mutual_key_proof_and_pake_confirm_the_same_channel() {
    let server_key =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([10; 32]))).unwrap();
    let client_key =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([11; 32]))).unwrap();
    let device = client_key.public_spki();
    let gateway = server_key.public_spki();
    let code = ManualCode::parse(b"ABCD2345").unwrap();
    let invitation =
        ServerInvitation::register(&mut Entropy, &code, public().invitation(), gateway).unwrap();
    let (a, b) = streams();
    let (server_lengths, client_lengths) = message_lengths();
    let server = thread::spawn(move || {
        let mut transport = CryptoFixtureTransport::new(
            NativeTransport::accept(a, &server_key).unwrap(),
            server_lengths,
        );
        let context = transport.pairing_context(public()).unwrap();
        let request = transport.read_message().unwrap();
        let (attempt, response) = invitation.start(&mut Entropy, &request, context).unwrap();
        transport.write_message(&response).unwrap();
        let proof = attempt.finish(&transport.read_message().unwrap()).unwrap();
        transport.write_message(b"claimed").unwrap();
        assert_eq!(proof.device().bytes(), &device[12..]);
        proof.context().as_bytes().to_vec()
    });
    let mut transport = CryptoFixtureTransport::new(
        NativeTransport::connect(b, &client_key, GatewayTrust::ManualBootstrap).unwrap(),
        client_lengths,
    );
    let context = transport.pairing_context(public()).unwrap();
    let (attempt, request) = ClientAttempt::start(&mut Entropy, &code).unwrap();
    transport.write_message(&request).unwrap();
    let saved = AtomicBool::new(false);
    let finalization = attempt
        .finish(
            &mut Entropy,
            &code,
            &transport.read_message().unwrap(),
            &context,
            |authenticated| {
                assert_eq!(authenticated.public(), public());
                saved.store(true, Ordering::SeqCst);
                Ok(())
            },
        )
        .unwrap();
    assert!(saved.load(Ordering::SeqCst));
    transport.write_message(&finalization).unwrap();
    assert_eq!(transport.read_message().unwrap(), b"claimed");
    assert_eq!(server.join().unwrap(), context.as_bytes());
}
#[cfg(unix)]
#[test]
fn strict_pin_refuses_gateway_key_substitution() {
    let (a, b) = streams();
    let server_key =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([10; 32]))).unwrap();
    let client_key =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([11; 32]))).unwrap();
    let different = NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([12; 32])))
        .unwrap()
        .public_spki();
    let server = thread::spawn(move || NativeTransport::accept(a, &server_key).is_err());
    assert!(NativeTransport::connect(b, &client_key, GatewayTrust::Pinned(different)).is_err());
    assert!(server.join().unwrap());
}
#[test]
fn failed_binding_or_pending_save_never_releases_ke3() {
    let gateway =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([10; 32]))).unwrap();
    let device =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([11; 32]))).unwrap();
    let (server_channel, client_channel) = native_channels(&gateway, &device);
    let (_, other_client_channel) = native_channels(&gateway, &device);
    let code = ManualCode::parse(b"ABCD2345").unwrap();
    let invitation = ServerInvitation::register(
        &mut Entropy,
        &code,
        public().invitation(),
        gateway.public_spki(),
    )
    .unwrap();
    let context = server_channel.pairing_context(public()).unwrap();
    let other_channel = other_client_channel.pairing_context(public()).unwrap();
    let (client, request) = ClientAttempt::start(&mut Entropy, &code).unwrap();
    let (_, response) = invitation.start(&mut Entropy, &request, context).unwrap();
    let saved = AtomicBool::new(false);
    assert_eq!(
        client
            .finish(&mut Entropy, &code, &response, &other_channel, |_| {
                saved.store(true, Ordering::SeqCst);
                Ok(())
            })
            .unwrap_err(),
        PairingCryptoError::InvalidProof
    );
    assert!(!saved.load(Ordering::SeqCst));
    let context = client_channel.pairing_context(public()).unwrap();
    let (client, request) = ClientAttempt::start(&mut Entropy, &code).unwrap();
    let (_, response) = invitation
        .start(
            &mut Entropy,
            &request,
            server_channel.pairing_context(public()).unwrap(),
        )
        .unwrap();
    assert_eq!(
        client
            .finish(&mut Entropy, &code, &response, &context, |_| Err(
                PairingCryptoError::PendingStorage
            ))
            .unwrap_err(),
        PairingCryptoError::PendingStorage
    );
}

#[test]
fn public_native_context_preserves_profile_and_same_channel_binding() {
    let gateway =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([10; 32]))).unwrap();
    let device =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([11; 32]))).unwrap();
    let (server, client) = native_channels(&gateway, &device);
    let context = server.pairing_context(public()).unwrap();
    let client_context = client.pairing_context(public()).unwrap();
    assert_eq!(context.as_bytes(), client_context.as_bytes());
    assert_eq!(context.public(), public());
    assert_eq!(context.gateway_spki(), gateway.public_spki());
    assert_eq!(context.device_spki(), device.public_spki());
    assert_eq!(context.as_bytes().len(), 335);
    let mut bytes = context.as_bytes();
    let mut parts = Vec::new();
    while !bytes.is_empty() {
        let (size, rest) = bytes.split_at(2);
        let size = usize::from(u16::from_be_bytes(size.try_into().unwrap()));
        let (part, rest) = rest.split_at(size);
        parts.push(part);
        bytes = rest;
    }
    assert_eq!(parts.len(), 12);
    assert_eq!(parts[0], b"nessa-device-pairing-v1");
    assert_eq!(
        parts[1],
        b"opaque-ke-4.0.1/ristretto255/tripledh/sha512/argon2id-19-m65536-t3-p4-o64"
    );
    assert_eq!(parts[2], b"manual");
    assert_eq!(parts[3], gateway.public_spki());
    assert_eq!(parts[4], device.public_spki());
    assert_eq!(parts[5], public().invitation().bytes());
    assert_eq!(parts[6], public().attempt().bytes());
    assert_eq!(parts[7], public().expiry_ms().to_be_bytes());
    assert_eq!(parts[8], public().consent().bytes());
    assert_eq!(parts[9], public().generation().to_be_bytes());
    assert_eq!(parts[10], public().class().as_bytes());
    assert_eq!(parts[11].len(), 32);
}

struct CoherentLogin {
    code: ManualCode,
    context: PairingContext,
    request: Vec<u8>,
    response: Vec<u8>,
    client: ClientAttempt,
    server: ServerAttempt,
    channel: NativeTransport<TcpStream>,
}
fn coherent_login() -> CoherentLogin {
    let gateway =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([10; 32]))).unwrap();
    let device =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([11; 32]))).unwrap();
    let (server_channel, channel) = native_channels(&gateway, &device);
    let code = ManualCode::parse(b"ABCD2345").unwrap();
    let invitation = ServerInvitation::register(
        &mut Entropy,
        &code,
        public().invitation(),
        gateway.public_spki(),
    )
    .unwrap();
    let context = channel.pairing_context(public()).unwrap();
    let (client, request) = ClientAttempt::start(&mut Entropy, &code).unwrap();
    let (server, response) = invitation
        .start(
            &mut Entropy,
            &request,
            server_channel.pairing_context(public()).unwrap(),
        )
        .unwrap();
    CoherentLogin {
        code,
        context,
        request,
        response,
        client,
        server,
        channel,
    }
}

#[test]
fn credential_request_bound_refuses_padded_valid_ke1() {
    let code = ManualCode::parse(b"ABCD2345").unwrap();
    let (_client, request) = ClientAttempt::start(&mut Entropy, &code).unwrap();
    let original = credential_request_fingerprint(&request).unwrap();
    let mut padded = request.clone();
    padded.resize(MAX_ENROLLMENT_MESSAGE_BYTES + 1, 0);
    assert_eq!(&padded[..request.len()], request.as_slice());
    assert_eq!(
        credential_request_fingerprint(&padded),
        Err(PairingCryptoError::InvalidProof)
    );
    assert_eq!(credential_request_fingerprint(&request).unwrap(), original);
}

#[test]
fn client_response_bound_refuses_padded_valid_ke2_before_pending_save() {
    for padded in [true, false] {
        let login = coherent_login();
        assert!(login.response.len() <= MAX_ENROLLMENT_MESSAGE_BYTES);
        let mut response = login.response.clone();
        if padded {
            response.resize(MAX_ENROLLMENT_MESSAGE_BYTES + 1, 0);
        }
        assert_eq!(&response[..login.response.len()], login.response.as_slice());
        let saved = AtomicBool::new(false);
        let result = login.client.finish(
            &mut Entropy,
            &login.code,
            &response,
            &login.context,
            |authenticated| {
                assert_eq!(authenticated.as_bytes(), login.context.as_bytes());
                saved.store(true, Ordering::SeqCst);
                Ok(())
            },
        );
        if padded {
            assert_eq!(result, Err(PairingCryptoError::InvalidProof));
            assert!(!saved.load(Ordering::SeqCst));
        } else {
            let finalization = result.unwrap();
            assert!(saved.load(Ordering::SeqCst));
            let confirmed = login
                .server
                .finish(&finalization)
                .unwrap_or_else(|error| panic!("exact original KE3 failed: {error}"));
            assert_eq!(confirmed.context().public(), public());
            assert_eq!(confirmed.context().as_bytes(), login.context.as_bytes());
            assert_eq!(
                confirmed.proof().input(),
                credential_request_fingerprint(&login.request).unwrap()
            );
        }
    }
}

#[test]
fn server_finalization_bound_refuses_padded_valid_ke3() {
    for padded in [true, false] {
        let login = coherent_login();
        let saved = AtomicBool::new(false);
        let finalization = login
            .client
            .finish(
                &mut Entropy,
                &login.code,
                &login.response,
                &login.context,
                |_| {
                    saved.store(true, Ordering::SeqCst);
                    Ok(())
                },
            )
            .unwrap();
        assert!(saved.load(Ordering::SeqCst));
        assert!(finalization.len() <= MAX_ENROLLMENT_MESSAGE_BYTES);
        let mut message = finalization.clone();
        if padded {
            message.resize(MAX_ENROLLMENT_MESSAGE_BYTES + 1, 0);
        }
        assert_eq!(&message[..finalization.len()], finalization.as_slice());
        let result = login.server.finish(&message);
        if padded {
            assert!(matches!(result, Err(PairingCryptoError::InvalidProof)));
        } else {
            let confirmed =
                result.unwrap_or_else(|error| panic!("exact original KE3 failed: {error}"));
            assert_eq!(confirmed.context().public(), public());
            assert_eq!(confirmed.context().as_bytes(), login.context.as_bytes());
            assert_eq!(
                confirmed.proof().input(),
                credential_request_fingerprint(&login.request).unwrap()
            );
            assert_eq!(
                confirmed.device().bytes(),
                &login.context.device_spki()[12..]
            );
        }
    }
}

#[test]
fn server_start_uses_original_request_decoder_refusal() {
    let CoherentLogin {
        code,
        context,
        request,
        channel,
        ..
    } = coherent_login();
    let invitation = ServerInvitation::register(
        &mut Entropy,
        &code,
        public().invitation(),
        context.gateway_spki(),
    )
    .unwrap();
    let mut padded = request.clone();
    padded.resize(super::MAX_ENROLLMENT_MESSAGE_BYTES + 1, 0);
    let same_context = channel.pairing_context(public()).unwrap();
    assert!(matches!(
        invitation.start(&mut Entropy, &padded, same_context),
        Err(PairingCryptoError::InvalidProof)
    ));
    assert!(invitation.start(&mut Entropy, &request, context).is_ok());
}
