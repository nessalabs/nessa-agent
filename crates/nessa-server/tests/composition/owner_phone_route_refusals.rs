//! Real disposable mounted refusal effects; private bytes are never assertion output.
use super::*;
use nessa_auth::{
    application::credential_admin::RevokeCredentialRequest,
    domain::{CredentialId, PrincipalId},
};
use tokio::task::JoinHandle;
async fn mounted(state: ProductRouteState) -> (SocketAddr, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, crate::server::entrypoint::http::router(state))
            .await
            .unwrap()
    });
    (address, task)
}
pub(super) fn receiver_effects(namespace: &Namespace) -> i64 {
    let path = crate::composition::local_auth::receiver_journal(&namespace.root());
    let connection = nessa_local_database::rusqlite::Connection::open_with_flags(
        path,
        nessa_local_database::rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    connection
        .query_row("SELECT COUNT(*) FROM receiver_transitions", [], |row| {
            row.get(0)
        })
        .unwrap()
}
fn unchanged(path: &std::path::Path, before: &[u8]) {
    assert!(
        std::fs::read(path).unwrap() == before,
        "refused setup must preserve registry bytes without exposing private contents"
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn phone_authorization_corrupted_signed_fields_refuse_without_registry_effects() {
    let namespace = Namespace::new();
    let registry = namespace.registry();
    let token = credential(&registry);
    let inputs = namespace.inputs(registry.clone());
    let receivers = inputs.receivers.clone();
    let (prepared, commands, _peers) = prepare(&loopback(), inputs).await.unwrap();
    let commands = Arc::new(commands);
    let state = product(registry.clone()).with_pairing(commands);
    let bound = bind(
        prepared,
        RuntimeDependencies::default().clock,
        state.clone(),
    )
    .await
    .unwrap();
    let (address, serving) = mounted(state).await;
    tokio::task::block_in_place(|| {
        let mut client = ProductClient::connect(address, &token);
        let original = signed(&mut client, 62);
        let path = namespace.root().join("auth/credentials.json");
        let before = std::fs::read(&path).unwrap();
        let journal_before = receiver_effects(&namespace);
        for field in [
            "signature",
            "deviceKey",
            "requestId",
            "gatewayKey",
            "source",
        ] {
            let mut changed = original.clone();
            match field {
                "signature" => {
                    changed["signature"][0] =
                        json!((changed["signature"][0].as_u64().unwrap() + 1) % 256)
                }
                "deviceKey" => {
                    changed["original"]["deviceKey"][0] =
                        json!((changed["original"]["deviceKey"][0].as_u64().unwrap() + 1) % 256)
                }
                "requestId" => changed["original"]["requestId"][0] = json!(63),
                "gatewayKey" => {
                    changed["original"]["discovery"]["gatewayKey"][43] = json!(
                        (changed["original"]["discovery"]["gatewayKey"][43]
                            .as_u64()
                            .unwrap()
                            + 1)
                            % 256
                    )
                }
                _ => {
                    changed["original"]["discovery"]["source"]["credentialId"] =
                        json!("another-bootstrap")
                }
            }
            assert_eq!(
                client.refused("phone.authorize", changed),
                if field == "source" {
                    "forbidden"
                } else {
                    "pairing_unavailable"
                },
                "{field}"
            );
            unchanged(&path, &before);
            assert_eq!(receiver_effects(&namespace), journal_before);
            assert!(registry.pending_pairings().unwrap().is_empty());
            assert!(receivers
                .binding(&CredentialId::new("phone-root").unwrap())
                .unwrap()
                .is_none());
        }
    });
    serving.abort();
    let _ = serving.await;
    drop(bound);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn phone_authorization_valid_signature_cannot_replace_actual_discovery_destination() {
    let namespace = Namespace::new();
    let registry = namespace.registry();
    let token = credential(&registry);
    let (prepared, commands, _peers) = prepare(&loopback(), namespace.inputs(registry.clone()))
        .await
        .unwrap();
    let commands = Arc::new(commands);
    let state = product(registry.clone()).with_pairing(commands);
    let bound = bind(
        prepared,
        RuntimeDependencies::default().clock,
        state.clone(),
    )
    .await
    .unwrap();
    let (address, serving) = mounted(state).await;
    tokio::task::block_in_place(|| {
        let mut client = ProductClient::connect(address, &token);
        let original = signed(&mut client, 64);
        let path = namespace.root().join("auth/credentials.json");
        let before = std::fs::read(&path).unwrap();
        let journal_before = receiver_effects(&namespace);
        for alternate_pin in [false, true] {
            let saved =
                owner_phone::request(serde_json::from_value(original["original"].clone()).unwrap())
                    .unwrap();
            let identity =
                NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([64; 32]))).unwrap();
            let pin = if alternate_pin {
                NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([11; 32])))
                    .unwrap()
                    .public_spki()
            } else {
                *saved.gateway_pin()
            };
            let endpoint = if alternate_pin {
                saved.endpoint()
            } else {
                let mut address = saved.endpoint();
                address.set_port(if address.port() == 65535 {
                    65534
                } else {
                    address.port() + 1
                });
                address
            };
            let changed = nessa_auth::domain::pairing::OwnerPhoneRequest::new(
                saved.id(),
                saved.source().clone(),
                pin,
                endpoint,
                saved.device_key(),
            )
            .unwrap();
            let params = PhoneAuthorizeParams {
                original: owner_phone::original(&changed).unwrap(),
                signature: identity
                    .sign_owner_phone_request(&changed)
                    .unwrap()
                    .into_iter()
                    .map(u64::from)
                    .collect(),
            };
            assert_eq!(
                client.refused("phone.authorize", serde_json::to_value(params).unwrap()),
                "pairing_conflict"
            );
            unchanged(&path, &before);
            assert_eq!(receiver_effects(&namespace), journal_before);
        }
    });
    serving.abort();
    let _ = serving.await;
    drop(bound);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn phone_existing_socket_revocation_cannot_commit_a_saved_signed_original() {
    let namespace = Namespace::new();
    let registry = namespace.registry();
    let token = credential(&registry);
    let (prepared, commands, _peers) = prepare(&loopback(), namespace.inputs(registry.clone()))
        .await
        .unwrap();
    let commands = Arc::new(commands);
    let state = product(registry.clone()).with_pairing(commands);
    let bound = bind(
        prepared,
        RuntimeDependencies::default().clock,
        state.clone(),
    )
    .await
    .unwrap();
    let (address, serving) = mounted(state).await;
    tokio::task::block_in_place(|| {
        let mut client = ProductClient::connect(address, &token);
        let params = signed(&mut client, 65);
        registry
            .revoke_sync(RevokeCredentialRequest {
                request_id: "revoke-mounted-bootstrap".into(),
                issuer_principal_id: "owner".into(),
                credential_id: "phone-root".into(),
                revoked_at: SystemClock.unix_seconds(),
            })
            .unwrap();
        let path = namespace.root().join("auth/credentials.json");
        let before = std::fs::read(&path).unwrap();
        let journal_before = receiver_effects(&namespace);
        let close = client
            .call_response_or_close("phone.authorize", params)
            .expect_err("revocation closes the socket");
        let reason =
            nessa_protocol::product_contract::generated::SessionCloseReason::CredentialRevoked;
        assert_eq!(u16::from(close.code), reason.web_socket_code());
        let termination: nessa_protocol::product::generated::SessionTermination =
            serde_json::from_str(&close.reason).unwrap();
        assert_eq!(termination.code, reason);
        assert!(!termination.retryable);
        assert_eq!(termination.retry_after_ms, None);
        unchanged(&path, &before);
        assert_eq!(receiver_effects(&namespace), journal_before);
        assert!(registry.pending_pairings().unwrap().is_empty());
    });
    serving.abort();
    let _ = serving.await;
    drop(bound);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn phone_status_keeps_original_revocation_after_actual_rebind_back() {
    let namespace = Namespace::new();
    let registry = namespace.registry();
    let token = credential(&registry);
    let inputs = namespace.inputs(registry.clone());
    let receivers = inputs.receivers.clone();
    let (prepared, commands, _peers) = prepare(&loopback(), inputs).await.unwrap();
    let commands = Arc::new(commands);
    let state = product(registry.clone()).with_pairing(commands);
    let bound = bind(
        prepared,
        RuntimeDependencies::default().clock,
        state.clone(),
    )
    .await
    .unwrap();
    let (address, serving) = mounted(state).await;
    let initial = tokio::task::block_in_place(|| {
        let mut client = ProductClient::connect(address, &token);
        let params = signed(&mut client, 66);
        client.ok("phone.authorize", params)
    });
    let device = CredentialId::new(initial["credentialId"].as_str().unwrap()).unwrap();
    let original = receivers.binding(&device).unwrap().unwrap();
    let owner = PrincipalId::new("owner").unwrap();
    for (replacement, active, request) in [
        (None, false, "mounted-revoke-original"),
        (
            Some(CredentialId::new("mounted-replacement").unwrap()),
            true,
            "mounted-regrant-other",
        ),
        (None, false, "mounted-revoke-other"),
        (Some(device.clone()), true, "mounted-regrant-back"),
    ] {
        receivers
            .change(
                original.receiver_id.clone(),
                replacement,
                active,
                owner.clone(),
                request.into(),
            )
            .await
            .unwrap();
    }
    let regranted = receivers.binding(&device).unwrap().unwrap();
    assert!(regranted.active && regranted.access_epoch > original.access_epoch);
    let path = namespace.root().join("auth/credentials.json");
    let before = std::fs::read(&path).unwrap();
    let journal_before = receiver_effects(&namespace);
    tokio::task::block_in_place(|| {
        let mut client = ProductClient::connect(address, &token);
        assert_eq!(client.refused("phone.status",json!({"requestId":initial["original"]["requestId"],"originalDigest":initial["originalDigest"]})),"pairing_unavailable");
    });
    unchanged(&path, &before);
    assert_eq!(receiver_effects(&namespace), journal_before);
    assert_eq!(receivers.binding(&device).unwrap(), Some(regranted));
    serving.abort();
    let _ = serving.await;
    drop(bound);
}
