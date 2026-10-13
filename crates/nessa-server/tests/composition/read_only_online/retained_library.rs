//! The public no-CLI retained API reads an actual paired owner's canonical data.
use super::fixtures::{Gateway, Setup};
use nessa_auth::{
    adapters::pairing::{FilePairingState, NativeIdentity},
    application::{pairing::ClientPendingStore, ports::Clock as WallClock},
};
use nessa_client_core::{
    pairing::NativeEnrollmentClient,
    retained::{ReadStop, ReaderAccess, ReaderRole, RetainedCache, RetainedTranscript},
};
use nessa_protocol::{
    clock::Clock as MonotonicClock, conversation::domain::ConversationId,
    pairing::wire::NativePairingStatus,
};
use serde_json::Value;
use std::{
    net::{SocketAddr, TcpStream},
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::runtime::Builder;
use uuid::Uuid;

struct Time(Instant);
impl MonotonicClock for Time {
    fn elapsed_ms(&self) -> u64 {
        self.0.elapsed().as_millis() as u64
    }
}
struct Wall;
impl WallClock for Wall {
    fn unix_milliseconds(&self) -> u64 {
        123_000
    }
}

#[test]
fn paired_owner_library_read_keeps_real_checkpoints_and_offline_views_across_restart() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let gateway = Gateway::start(&root);
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let profile: Value =
        serde_json::from_slice(&std::fs::read(root.join("profile.json")).unwrap()).unwrap();
    let address: SocketAddr = profile["gatewayAddress"].as_str().unwrap().parse().unwrap();
    let state = Arc::new(
        FilePairingState::open(
            Path::new(profile["stateRoot"].as_str().unwrap()),
            Path::new(profile["stateDirectory"].as_str().unwrap()),
        )
        .unwrap(),
    );
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    let device = NativeEnrollmentClient::new(state.clone(), Arc::new(Time(Instant::now())));
    let status = runtime
        .block_on(device.status(TcpStream::connect(address).unwrap(), None))
        .unwrap();
    runtime.block_on(device.shutdown());
    let NativePairingStatus::Active {
        receiver,
        access_epoch,
        ..
    } = status
    else {
        panic!("paired device must be active")
    };
    let saved = state.load_credential().unwrap().unwrap();
    let credential = saved.credential().as_str().to_owned();
    let (key, pin, _) = saved.into_enrollment().into_parts();
    let identity = NativeIdentity::restore(key).unwrap();
    let cache_root = directory.path().join("cache");
    nessa_local_storage::create_directory(&cache_root).unwrap();
    let path = cache_root.join("owner.sqlite3");
    let mut cache =
        RetainedCache::open(&path, Arc::new(Wall), Arc::new(Time(Instant::now()))).unwrap();
    let read = |cache: &mut RetainedCache| {
        cache.read(
            ReaderAccess {
                role: ReaderRole::DeviceOwner,
                address,
                identity: &identity,
                pin,
                credential: &credential,
                receiver: receiver.as_str(),
                access_epoch,
                client_id: "retained-library-test",
            },
            &ReadStop::new(),
            Duration::from_secs(5),
            8,
            &mut Vec::new(),
        )
    };
    let report = read(&mut cache).unwrap();
    assert!(report.complete);
    assert!(report.conversations >= 2);
    let scope = cache
        .saved_catalogue_scope(receiver.as_str())
        .unwrap()
        .unwrap();
    let page = cache
        .catalogue_page(
            receiver.as_str(),
            scope.origin().as_str(),
            scope.stream().as_str(),
            None,
            16,
        )
        .unwrap()
        .unwrap();
    assert!(page
        .entries()
        .iter()
        .any(|entry| entry.manifest().key.id.as_str() == setup.conversation));
    let conversation = ConversationId::new(&setup.conversation).unwrap();
    let RetainedTranscript::Retained(original_snapshot) = cache
        .transcript_view(
            receiver.as_str(),
            scope.origin().as_str(),
            &conversation,
            Uuid::nil(),
        )
        .unwrap()
    else {
        panic!("actual owner transcript was not retained")
    };
    let (progress, view) = (original_snapshot.progress(), original_snapshot.view());
    assert!(progress.downloaded() > 0);
    assert_eq!(progress.downloaded(), progress.applied());
    assert!(progress.facts() > 0);
    let original = serde_json::to_value(view).unwrap();
    drop(cache);
    drop(gateway);
    let mut offline =
        RetainedCache::open(&path, Arc::new(Wall), Arc::new(Time(Instant::now()))).unwrap();
    let RetainedTranscript::Retained(saved_snapshot) = offline
        .transcript_view(
            receiver.as_str(),
            scope.origin().as_str(),
            &conversation,
            Uuid::nil(),
        )
        .unwrap()
    else {
        panic!("offline reopen lost retained evidence")
    };
    let (saved_progress, saved_view) = (saved_snapshot.progress(), saved_snapshot.view());
    assert_eq!(saved_progress, progress);
    assert_eq!(serde_json::to_value(saved_view).unwrap(), original);
    drop(offline);
    let _restarted = Gateway::start(&root);
    let mut resumed =
        RetainedCache::open(&path, Arc::new(Wall), Arc::new(Time(Instant::now()))).unwrap();
    let report = read(&mut resumed).unwrap();
    assert!(report.complete);
    assert!(
        !report.moved,
        "unchanged retained walk resumes without replaying records"
    );
}
