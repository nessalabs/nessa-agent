//! Physical receiver dispatch authority using actual enrollment and registry leases.
use super::{
    activation,
    support::{pending, Fixture, WAIT},
};
use nessa_auth::{
    adapters::pairing::{ManualCode, OsEntropy},
    application::pairing::{PairingStore, RuntimeEnd},
    domain::pairing::{ConsentClass, PairingPhase},
};
use nessa_client_core::pairing::NativeEnrollmentClient;
use nessa_server::{
    app::dependencies::RuntimeDependencies,
    device_pairing::{
        application::{PairingReceivers, ReceiverDispatch, ReceiverError, ReceiverRequest},
        infrastructure::ConversationReceivers,
    },
};
use std::{net::TcpStream, sync::atomic::Ordering};

async fn staging() -> (Fixture, nessa_auth::domain::pairing::InvitationId) {
    let (fixture, switches) = activation::fixture().await;
    let created = fixture
        .gateway
        .create(fixture.session.clone(), ConsentClass::DeviceRead, OsEntropy)
        .await
        .unwrap();
    let (address, stop, listener, _) = fixture.listener().await;
    let (_, store) = pending(fixture.directory.path(), "dispatch-device");
    let client = NativeEnrollmentClient::new(store, RuntimeDependencies::default().clock);
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    tokio::time::timeout(
        WAIT,
        client.enroll(TcpStream::connect(address).unwrap(), code, OsEntropy),
    )
    .await
    .unwrap()
    .unwrap();
    let id = created.record().id();
    let (_, key) = fixture
        .registry
        .read_pairing(id)
        .unwrap()
        .claim_binding()
        .unwrap();
    switches.lose_pair_answer.store(true, Ordering::SeqCst);
    let approved = fixture
        .gateway
        .approve(&fixture.session, id, key, OsEntropy)
        .await
        .unwrap();
    assert_eq!(approved.record.phase(), PairingPhase::Staging);
    assert!(approved.record.receiver_binding().is_none());
    client.shutdown().await;
    stop.send(()).unwrap();
    tokio::time::timeout(WAIT, listener)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    (fixture, id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receiver_dispatch_requires_kind_and_remembered_terminal_receipt() {
    let (fixture, id) = staging().await;
    let stage = fixture.registry.clone().acquire_stage(id).unwrap();
    let real = ConversationReceivers::new(fixture.receivers.clone());
    let dispatch = ReceiverDispatch::for_pair(&stage).unwrap();
    let selector: ReceiverRequest = (*dispatch).clone();
    let original = real.paired(&selector).unwrap().unwrap();
    assert!(ReceiverDispatch::for_fence(&stage).is_err());
    assert_eq!(
        real.fence(&dispatch, original.receiver(), original.epoch()),
        Err(ReceiverError::Conflict)
    );
    assert_eq!(
        real.holding(&selector, original.receiver(), original.epoch())
            .unwrap(),
        Some(original.epoch())
    );
    fixture
        .registry
        .remember_receiver(id, &original, fixture.time.as_ref())
        .unwrap();
    assert!(ReceiverDispatch::for_pair(&stage).is_err());
    assert!(ReceiverDispatch::for_fence(&stage).is_err());
    fixture.time.set(stage.current().unwrap().expires_at_ms());
    fixture
        .registry
        .end_pairing(id, RuntimeEnd::Expired, fixture.time.as_ref())
        .unwrap();
    let fence = ReceiverDispatch::for_fence(&stage).unwrap();
    assert_eq!(real.pair(&fence), Err(ReceiverError::Conflict));
    assert_eq!(
        real.holding(&selector, original.receiver(), original.epoch())
            .unwrap(),
        Some(original.epoch())
    );
    let fenced = real
        .fence(&fence, original.receiver(), original.epoch())
        .unwrap();
    assert!(fenced.epoch() > original.epoch());
    assert_eq!(
        real.holding(&selector, original.receiver(), original.epoch())
            .unwrap(),
        None
    );
    assert_eq!(real.paired(&selector).unwrap(), Some(original));
    drop(fence);
    drop(dispatch);
    drop(stage);
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receiver_dispatch_original_pair_survives_terminal_and_blocks_new_pair() {
    let (fixture, id) = staging().await;
    let stage = fixture.registry.clone().acquire_stage(id).unwrap();
    let real = ConversationReceivers::new(fixture.receivers.clone());
    let admitted = ReceiverDispatch::for_pair(&stage).unwrap();
    let selector: ReceiverRequest = (*admitted).clone();
    let original = real.paired(&selector).unwrap().unwrap();
    fixture.time.set(stage.current().unwrap().expires_at_ms());
    fixture
        .registry
        .end_pairing(id, RuntimeEnd::Expired, fixture.time.as_ref())
        .unwrap();
    assert!(ReceiverDispatch::for_pair(&stage).is_err());
    assert!(ReceiverDispatch::for_fence(&stage).is_err());
    assert!(fixture.registry.clone().acquire_stage(id).is_err());
    // An already admitted physical dispatch retains its original correlation.
    assert_eq!(real.pair(&admitted).unwrap(), original);
    let terminal = fixture
        .registry
        .remember_receiver(id, &original, fixture.time.as_ref())
        .unwrap();
    assert_eq!(terminal.phase(), PairingPhase::Terminal);
    assert!(terminal.cleanup_pending());
    drop(admitted);
    drop(stage);
    let cleanup = fixture.registry.clone().acquire_stage(id).unwrap();
    let fence = ReceiverDispatch::for_fence(&cleanup).unwrap();
    real.fence(&fence, original.receiver(), original.epoch())
        .unwrap();
    assert_eq!(
        real.holding(&selector, original.receiver(), original.epoch())
            .unwrap(),
        None
    );
    drop(fence);
    drop(cleanup);
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receiver_dispatch_fence_cannot_replace_original_receipt_with_regranted_epoch() {
    let (fixture, id) = staging().await;
    let stage = fixture.registry.clone().acquire_stage(id).unwrap();
    let real = ConversationReceivers::new(fixture.receivers.clone());
    let pair = ReceiverDispatch::for_pair(&stage).unwrap();
    let selector: ReceiverRequest = (*pair).clone();
    let original = real.paired(&selector).unwrap().unwrap();
    fixture
        .registry
        .remember_receiver(id, &original, fixture.time.as_ref())
        .unwrap();
    fixture.time.set(stage.current().unwrap().expires_at_ms());
    fixture
        .registry
        .end_pairing(id, RuntimeEnd::Expired, fixture.time.as_ref())
        .unwrap();
    let fence = ReceiverDispatch::for_fence(&stage).unwrap();
    let wrong_receiver = nessa_auth::domain::ResourceId::new("not-original-receiver").unwrap();
    for (receiver, epoch) in [
        (&wrong_receiver, original.epoch()),
        (original.receiver(), original.epoch() + 1),
    ] {
        assert_eq!(
            real.fence(&fence, receiver, epoch),
            Err(ReceiverError::Conflict)
        );
        assert_eq!(
            real.holding(&selector, original.receiver(), original.epoch())
                .unwrap(),
            Some(original.epoch())
        );
    }
    let revoked = fixture
        .receivers
        .change(
            original.receiver().as_str().into(),
            None,
            false,
            selector.owner().clone(),
            "dispatch-revoke".into(),
        )
        .await
        .unwrap();
    // Receiver regrant requires a changed credential. Exercise the canonical
    // P46 path through a replacement and its revocation before returning.
    for (replacement, active, request) in [
        (
            Some(nessa_auth::domain::CredentialId::new("dispatch-replacement").unwrap()),
            true,
            "dispatch-regrant-other",
        ),
        (None, false, "dispatch-revoke-other"),
    ] {
        fixture
            .receivers
            .change(
                original.receiver().as_str().into(),
                replacement,
                active,
                selector.owner().clone(),
                request.into(),
            )
            .await
            .unwrap();
    }
    let regranted = fixture
        .receivers
        .change(
            original.receiver().as_str().into(),
            Some(original.credential().clone()),
            true,
            selector.owner().clone(),
            "dispatch-regrant".into(),
        )
        .await
        .unwrap();
    assert!(regranted.access_epoch > revoked.access_epoch);
    // Substituting the new epoch would conceal the intervening original revocation.
    assert_eq!(
        real.fence(&fence, original.receiver(), regranted.access_epoch),
        Err(ReceiverError::Conflict)
    );
    assert_eq!(
        fixture.receivers.binding(original.credential()).unwrap(),
        Some(regranted.clone())
    );
    let outcome = real
        .fence(&fence, original.receiver(), original.epoch())
        .unwrap();
    assert_eq!(outcome.receiver(), original.receiver());
    assert_eq!(outcome.epoch(), revoked.access_epoch);
    assert_eq!(outcome.credential(), original.credential());
    assert_eq!(
        fixture.receivers.binding(original.credential()).unwrap(),
        Some(regranted)
    );
    assert_eq!(real.paired(&selector).unwrap(), Some(original));
    drop(fence);
    drop(pair);
    drop(stage);
    fixture.gateway.shutdown().await;
}
