//! Actual pinned TLS recovery of a signed original, without registry or receiver effects.
use super::*;
use nessa_auth::adapters::pairing::{GatewayTrust, NativeTransport};
use nessa_auth::domain::CredentialId;
use nessa_protocol::pairing::{
    wire::{
        decode_reply, encode_request, NativePairingReply, NativePairingRequest, NativePairingStatus,
    },
    EnrollmentChannel,
};
async fn mounted(state: ProductRouteState) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, crate::server::entrypoint::http::router(state))
            .await
            .unwrap()
    });
    (address, task)
}
fn recover(
    original: &nessa_auth::domain::pairing::OwnerPhoneRequest,
    nonce: u8,
    id: OwnerPhoneRequestId,
    digest: [u8; 32],
) -> NativePairingReply {
    let identity =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([nonce; 32]))).unwrap();
    let socket = std::net::TcpStream::connect(original.endpoint()).unwrap();
    socket.set_read_timeout(Some(WAIT)).unwrap();
    socket.set_write_timeout(Some(WAIT)).unwrap();
    let transport = NativeTransport::connect(
        socket,
        &identity,
        GatewayTrust::Pinned(*original.gateway_pin()),
    )
    .unwrap();
    let mut channel = EnrollmentChannel::new(transport);
    channel
        .send_envelope(
            &encode_request(&NativePairingRequest::RecoverOwnerPhone {
                request_id: id,
                digest,
            })
            .unwrap(),
        )
        .unwrap();
    decode_reply(&channel.receive_envelope().unwrap()).unwrap()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_tls_recovery_is_exact_read_only_and_survives_bootstrap_revocation() {
    let namespace = Namespace::new();
    let registry = namespace.registry();
    let token = credential(&registry);
    let inputs = namespace.inputs(registry.clone());
    let receivers = inputs.receivers.clone();
    let (prepared, commands, _) = prepare(&loopback(), inputs).await.unwrap();
    let state = product(registry.clone()).with_pairing(Arc::new(commands));
    let bound = bind(
        prepared,
        RuntimeDependencies::default().clock,
        state.clone(),
    )
    .await
    .unwrap();
    let (failure, failed) = watch::channel(None);
    let mut running = start(bound, failure);
    let (address, serving) = mounted(state).await;
    let (params, active) = tokio::task::block_in_place(|| {
        let mut client = ProductClient::connect(address, &token);
        let params = signed(&mut client, 71);
        let active = client.ok("phone.authorize", params.clone());
        (params, active)
    });
    let original =
        owner_phone::request(serde_json::from_value(params["original"].clone()).unwrap()).unwrap();
    let digest = owner_phone::octets::<32>(
        &serde_json::from_value::<Vec<u64>>(active["originalDigest"].clone()).unwrap(),
    )
    .unwrap();
    let device = CredentialId::new(active["credentialId"].as_str().unwrap()).unwrap();
    let path = namespace.root().join("auth/credentials.json");
    let verify = |stage: &str| {
        let bytes = std::fs::read(&path).unwrap();
        let effects = super::refusals::receiver_effects(&namespace);
        let binding = receivers.binding(&device).unwrap();
        tokio::task::block_in_place(|| {
            let NativePairingReply::OwnerPhoneRecovery {
                request_id,
                digest: actual,
                status,
            } = recover(&original, 71, original.id(), digest)
            else {
                panic!("exact original recovery refused at {stage}")
            };
            assert_eq!(request_id, original.id());
            assert_eq!(actual, digest);
            let public = status.public();
            assert_eq!(json!(*public.invitation().bytes()), active["invitationId"]);
            assert_eq!(json!(*public.consent().bytes()), active["consentId"]);
            assert_eq!(json!(public.generation()), active["generation"]);
            assert_eq!(json!(public.expiry_ms()), active["expiresAtMs"]);
            assert_eq!(public.attempt().bytes(), original.id().bytes());
            assert_eq!(
                json!(public.class().as_str()),
                params["original"]["discovery"]["consent"]["class"]
            );
            original
                .correlate_disclosure(status.consent().unwrap())
                .unwrap();
            let NativePairingStatus::Active {
                credential,
                receiver,
                access_epoch,
                ..
            } = status
            else {
                panic!("actual enrolled original not active")
            };
            assert_eq!(credential, device);
            let current = binding.as_ref().unwrap();
            assert_eq!(receiver.as_str(), current.receiver_id);
            assert_eq!(access_epoch, current.access_epoch);
            for (key, id, hash) in [
                (72, original.id(), digest),
                (71, OwnerPhoneRequestId::new([72; 16]).unwrap(), digest),
                (71, original.id(), [0; 32]),
            ] {
                assert!(matches!(
                    recover(&original, key, id, hash),
                    NativePairingReply::Refused
                ));
            }
        });
        assert!(
            std::fs::read(&path).unwrap() == bytes,
            "recovery must not mutate registry"
        );
        assert_eq!(super::refusals::receiver_effects(&namespace), effects);
        assert_eq!(receivers.binding(&device).unwrap(), binding);
    };
    verify("before bootstrap revocation");
    registry
        .revoke_sync(
            nessa_auth::application::credential_admin::RevokeCredentialRequest {
                request_id: "recovery-bootstrap-revoke".into(),
                issuer_principal_id: "owner".into(),
                credential_id: "phone-root".into(),
                revoked_at: SystemClock.unix_seconds(),
            },
        )
        .unwrap();
    verify("after bootstrap revocation");
    let original_binding = receivers.binding(&device).unwrap().unwrap();
    let owner = nessa_auth::domain::PrincipalId::new("owner").unwrap();
    for (replacement, active, request) in [
        (None, false, "recovery-original-revoke"),
        (
            Some(CredentialId::new("recovery-replacement").unwrap()),
            true,
            "recovery-grant-other",
        ),
        (None, false, "recovery-other-revoke"),
        (Some(device.clone()), true, "recovery-regrant-back"),
    ] {
        receivers
            .change(
                original_binding.receiver_id.clone(),
                replacement,
                active,
                owner.clone(),
                request.into(),
            )
            .await
            .unwrap();
    }
    let current = receivers.binding(&device).unwrap().unwrap();
    assert!(current.active && current.access_epoch > original_binding.access_epoch);
    let bytes = std::fs::read(&path).unwrap();
    let effects = super::refusals::receiver_effects(&namespace);
    tokio::task::block_in_place(|| {
        assert!(matches!(
            recover(&original, 71, original.id(), digest),
            NativePairingReply::Refused
        ))
    });
    assert!(
        std::fs::read(&path).unwrap() == bytes,
        "historical recovery cannot mutate registry"
    );
    assert_eq!(super::refusals::receiver_effects(&namespace), effects);
    assert_eq!(receivers.binding(&device).unwrap().as_ref(), Some(&current));
    running.signal_stop();
    assert!(matches!(
        tokio::time::timeout(WAIT, running.join()).await,
        Ok(Ok(()))
    ));
    assert!(failed.borrow().is_none());
    serving.abort();
    let _ = serving.await;
}
