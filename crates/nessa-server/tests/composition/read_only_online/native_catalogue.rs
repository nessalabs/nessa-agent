//! A real native product child observes grant-scoped catalogue replacements.
use super::fixtures::{Gateway, Setup};
use nessa_device_client::{NativeDeviceSession, NativeProductCompletionKind, NativeProductSession};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

fn call(product: &NativeProductSession, method: &str, params: Value) -> Value {
    let ticket = product.request(method.into(), params.to_string()).unwrap();
    let completion = ticket.wait();
    assert_eq!(completion.kind(), NativeProductCompletionKind::Response);
    let frame: Value = serde_json::from_str(&completion.response_json().unwrap().unwrap()).unwrap();
    assert_eq!(frame["id"], ticket.id().to_string());
    assert_eq!(frame["ok"], true, "{frame}");
    frame["payload"].clone()
}

fn ids(list: &Value) -> BTreeSet<String> {
    let rows = list["conversations"].as_array().unwrap();
    let ids: BTreeSet<_> = rows
        .iter()
        .map(|row| row["conversationId"].as_str().unwrap().into())
        .collect();
    assert_eq!(ids.len(), rows.len(), "no repeated identity");
    ids
}

fn check(product: &NativeProductSession, expected: &BTreeSet<String>) {
    let list = call(product, "conversation.list", json!({}));
    assert_eq!(&ids(&list), expected);
    assert_eq!(list["complete"], true);
    let observed = call(product, "conversation.observe", json!({}));
    assert_eq!(&ids(&observed), expected);
    assert_eq!(observed["complete"], true);
    assert!(observed.get("cursor").is_none());
}

fn listed(product: &NativeProductSession, subscription: &str, expected: &BTreeSet<String>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            Instant::now() < deadline,
            "catalogue replacement within deadline"
        );
        let frames = product.events(100).unwrap().frames_json().unwrap();
        if frames.is_empty() {
            continue;
        }
        assert_eq!(frames.len(), 1, "one replacement for this durable change");
        let frame: Value = serde_json::from_str(&frames[0]).unwrap();
        assert_eq!(frame["event"], "conversation.listed", "{frame}");
        assert_eq!(frame["payload"]["subscriptionId"], subscription);
        assert_eq!(&ids(&frame["payload"]["list"]), expected);
        return;
    }
}

fn connect(root: &Path, instance: &str) -> (NativeDeviceSession, Arc<NativeProductSession>) {
    let profile: Value =
        serde_json::from_slice(&std::fs::read(root.join("profile.json")).unwrap()).unwrap();
    let native = NativeDeviceSession::open(
        profile["stateRoot"].as_str().unwrap().into(),
        profile["stateDirectory"].as_str().unwrap().into(),
        profile["gatewayAddress"].as_str().unwrap().into(),
        None,
    )
    .unwrap();
    let product = native.open_product(instance.into()).unwrap();
    let ready = product.wait_ready().unwrap();
    for method in [
        "conversation.list",
        "conversation.observe",
        "conversation.subscribeList",
    ] {
        assert!(ready
            .methods()
            .iter()
            .any(|advertised| advertised == method));
    }
    (native, product)
}

#[test]
fn native_paired_catalogue_stress() {
    let cycles = std::env::var("NESSA_PAIRED_CATALOGUE_CYCLES")
        .map_or(1, |value| value.parse::<usize>().unwrap());
    assert!((1..=100).contains(&cycles));
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let mut gateway = Gateway::start_catalogue(&root);
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let (native, product) = connect(&root, "catalogue-stress");
    let mut expected = BTreeSet::from([setup.conversation.clone(), setup.empty.clone()]);
    check(&product, &expected);
    let opened = call(&product, "conversation.subscribeList", json!({}));
    let subscription = opened["subscriptionId"].as_str().unwrap();
    listed(&product, subscription, &expected);
    for cycle in 0..cycles {
        gateway.act("hidden");
        assert!(product
            .events(300)
            .unwrap()
            .frames_json()
            .unwrap()
            .is_empty());
        for action in [
            "replace two",
            "replace three",
            "restore",
            "revoke",
            "restore",
            "archive",
            "unarchive",
        ] {
            gateway.act(action);
            match action {
                "replace two" | "revoke" | "archive" => {
                    expected.remove(&setup.conversation);
                }
                "restore" | "unarchive" => {
                    expected.insert(setup.conversation.clone());
                }
                "replace three" => {}
                _ => unreachable!(),
            }
            if action == "replace three" {
                assert!(product
                    .events(300)
                    .unwrap()
                    .frames_json()
                    .unwrap()
                    .is_empty());
            } else {
                listed(&product, subscription, &expected);
            }
            check(&product, &expected);
        }
        eprintln!(
            "native paired catalogue cycle {}/{} passed",
            cycle + 1,
            cycles
        );
    }
    call(
        &product,
        "conversation.unsubscribe",
        json!({"subscriptionId":subscription}),
    );
    product.close().unwrap();
    native.close().unwrap();
    let (native, product) = connect(&root, "catalogue-reconnect");
    check(&product, &expected);
    product.close().unwrap();
    native.close().unwrap();
}

/// A stand-in for the tunnel's edge: each WebSocket it takes is bridged to a
/// fresh TCP connection to `gateway`, and every one it takes is counted.
fn edge(gateway: std::net::SocketAddr) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    use nessa_client_core::tunnel::{bridge, websocket_config};
    let taken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let counted = taken.clone();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            listener.set_nonblocking(true).unwrap();
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::spawn(async move {
                    let websocket = tokio_tungstenite::accept_async_with_config(
                        stream,
                        Some(websocket_config()),
                    )
                    .await
                    .unwrap();
                    let socket = tokio::net::TcpStream::connect(gateway).await.unwrap();
                    bridge(websocket, socket).await;
                });
            }
        });
    });
    (origin, taken)
}

/// A phone away from its gateway's network: the address it paired on refuses
/// every dial, and its product connection goes through the tunnel it was told
/// about instead, under the same pinned TLS.
#[test]
fn a_phone_away_from_its_network_reaches_the_gateway_through_its_tunnel() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let _gateway = Gateway::start_catalogue(&root);
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let profile: Value =
        serde_json::from_slice(&std::fs::read(root.join("profile.json")).unwrap()).unwrap();
    let gateway: std::net::SocketAddr =
        profile["gatewayAddress"].as_str().unwrap().parse().unwrap();
    let (tunnel, taken) = edge(gateway);
    // Nothing listens here: every direct dial is refused at once.
    let away = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let native = NativeDeviceSession::open(
        profile["stateRoot"].as_str().unwrap().into(),
        profile["stateDirectory"].as_str().unwrap().into(),
        away.to_string(),
        Some(tunnel),
    )
    .unwrap();
    let product = native.open_product("away".into()).unwrap();
    product.wait_ready().unwrap();
    assert!(
        taken.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "the product connection came through the tunnel"
    );
    check(
        &product,
        &BTreeSet::from([setup.conversation.clone(), setup.empty.clone()]),
    );
    product.close().unwrap();
    native.close().unwrap();
}
