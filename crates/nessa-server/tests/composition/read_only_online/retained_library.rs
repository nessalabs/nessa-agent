//! The public no-CLI retained API reads an actual paired owner's canonical data.
use super::fixtures::{Gateway, Setup};
use nessa_auth::{
    adapters::pairing::{FilePairingState, NativeIdentity},
    application::{
        pairing::{
            ClientPendingStore, DeviceCredential, PairingWorkerFault, PendingEnrollment,
            PrivateKeyMaterial, PrivateStateError,
        },
        ports::Clock as WallClock,
    },
    domain::{pairing::PublicIntent, CredentialId, ResourceId},
};
use nessa_client_core::{
    pairing::NativeClientError,
    retained::{
        FeedEvent, ReadFailure, ReadStop, ReaderAccess, ReaderRole, RetainedCache,
        RetainedEnrollmentClient, RetainedTranscript,
    },
};
use nessa_protocol::{
    clock::Clock as MonotonicClock, conversation::domain::ConversationId,
    pairing::wire::NativePairingStatus, product::generated::ConversationViewCursor,
};
use serde_json::Value;
use std::{
    net::{SocketAddr, TcpStream},
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
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

struct RemovalState {
    state: Arc<dyn ClientPendingStore>,
    failure_after_removal: AtomicUsize,
}
impl ClientPendingStore for RemovalState {
    fn enrollment_key(&self) -> Result<Option<PrivateKeyMaterial>, PrivateStateError> {
        self.state.enrollment_key()
    }
    fn load_pending(&self) -> Result<Option<PendingEnrollment>, PrivateStateError> {
        self.state.load_pending()
    }
    fn load_credential(&self) -> Result<Option<DeviceCredential>, PrivateStateError> {
        self.state.load_credential()
    }
    fn save_pending(
        &self,
        key: &PrivateKeyMaterial,
        pin: &[u8; 44],
        intent: PublicIntent,
        expected: Option<PublicIntent>,
    ) -> Result<(), PrivateStateError> {
        self.state.save_pending(key, pin, intent, expected)
    }
    fn save_credential(
        &self,
        credential: &CredentialId,
        receiver: &ResourceId,
        expected: PublicIntent,
    ) -> Result<(), PrivateStateError> {
        self.state.save_credential(credential, receiver, expected)
    }
    fn end_enrollment(&self, expected: PublicIntent) -> Result<(), PrivateStateError> {
        self.state.end_enrollment(expected)?;
        match self.failure_after_removal.load(Ordering::SeqCst) {
            0 => Ok(()),
            1 => Err(PrivateStateError::Uncertain),
            2 => panic!("fixture private-state fault after unlink"),
            _ => unreachable!(),
        }
    }
}

#[test]
fn paired_owner_library_read_keeps_real_checkpoints_and_offline_views_across_restart() {
    verify_paired_lifecycle(0);
}
#[test]
fn paired_owner_library_keeps_purge_receipt_after_uncertain_credential_removal() {
    verify_paired_lifecycle(1);
}
#[test]
fn paired_owner_library_worker_fault_cannot_leak_purge_into_the_next_status() {
    verify_paired_lifecycle(2);
}
fn verify_paired_lifecycle(removal_mode: usize) {
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
    let cache_root = directory.path().join("cache");
    nessa_local_storage::create_directory(&cache_root).unwrap();
    let path = cache_root.join("owner.sqlite3");
    let removal = Arc::new(RemovalState {
        state: state.clone(),
        failure_after_removal: AtomicUsize::new(0),
    });
    let device = RetainedEnrollmentClient::new(
        removal.clone(),
        path.clone(),
        Arc::new(Wall),
        Arc::new(Time(Instant::now())),
    )
    .unwrap();
    let status = runtime
        .block_on(device.status(TcpStream::connect(address).unwrap(), None))
        .unwrap();
    assert!(status.purge().is_none());
    let NativePairingStatus::Active {
        receiver,
        access_epoch,
        ..
    } = status.enrollment().as_ref().unwrap()
    else {
        panic!("paired device must be active")
    };
    let saved = state.load_credential().unwrap().unwrap();
    let credential = saved.credential().as_str().to_owned();
    let (key, pin, _) = saved.into_enrollment().into_parts();
    let identity = NativeIdentity::restore(key).unwrap();

    let mut cache =
        RetainedCache::open(&path, Arc::new(Wall), Arc::new(Time(Instant::now()))).unwrap();
    let read = |cache: &mut RetainedCache| {
        cache.read(
            ReaderAccess {
                role: ReaderRole::DeviceOwner,
                address,
                relay: None,
                identity: &identity,
                pin,
                credential: &credential,
                receiver: receiver.as_str(),
                access_epoch: *access_epoch,
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
    drop(_restarted);
    let _revoked = Gateway::start_mode(&root, "revoke");
    removal
        .failure_after_removal
        .store(removal_mode, Ordering::SeqCst);
    let ended = runtime.block_on(device.status(TcpStream::connect(address).unwrap(), None));
    let purge = resumed.purge_receipt(receiver.as_str()).unwrap().unwrap();
    if removal_mode == 2 {
        assert!(matches!(
            ended,
            Err(NativeClientError::WorkerFault(PairingWorkerFault::Panic))
        ));
    } else {
        let ended = ended.unwrap();
        if removal_mode == 1 {
            assert_eq!(
                ended.enrollment().as_ref().unwrap_err(),
                &NativeClientError::Storage(PrivateStateError::Uncertain)
            );
        } else {
            assert!(matches!(
                ended.enrollment(),
                Ok(NativePairingStatus::Terminal { .. })
            ));
        }
        assert_eq!(ended.purge().unwrap().as_ref().unwrap(), &purge);
    }
    assert_eq!(purge.receiver().as_str(), receiver.as_str());
    assert!(purge.records() > 0);
    assert!(purge.transcripts() > 0);
    assert!(purge.catalogue_entries() >= 2);
    assert!(state.load_credential().unwrap().is_none());
    assert_eq!(
        resumed.purge_receipt(receiver.as_str()).unwrap().as_ref(),
        Some(&purge)
    );
    assert!(matches!(
        resumed
            .transcript_view(
                receiver.as_str(),
                scope.origin().as_str(),
                &conversation,
                Uuid::nil()
            )
            .unwrap(),
        RetainedTranscript::Deleted
    ));
    drop(resumed);
    let mut reopened =
        RetainedCache::open(&path, Arc::new(Wall), Arc::new(Time(Instant::now()))).unwrap();
    assert_eq!(
        reopened.purge_receipt(receiver.as_str()).unwrap().as_ref(),
        Some(&purge)
    );
    let absent = runtime
        .block_on(device.status(TcpStream::connect(address).unwrap(), None))
        .unwrap();
    assert_eq!(
        absent.enrollment().as_ref().unwrap_err(),
        &NativeClientError::NoPending
    );
    assert!(
        absent.purge().is_none(),
        "this operation cannot report the previous purge"
    );
    assert_eq!(
        reopened.purge_receipt(receiver.as_str()).unwrap().as_ref(),
        Some(&purge)
    );
    runtime.block_on(device.shutdown());
}

/// The subscription feed and the retained records fold report the same activity
/// for one granted conversation. The corpus is the semantic gateway's settled
/// suffix: while a permission is still open, the live agent and the retained
/// fold disagree on message status, which that fixture already records.
/// Revision, gateway time, title, approval mode and capabilities stay out of
/// the comparison.
#[test]
fn paired_owner_subscription_feed_matches_the_retained_records_view() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let mut gateway = Gateway::start_semantic(&root);
    gateway.act("settle");
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
    let cache_root = directory.path().join("cache");
    nessa_local_storage::create_directory(&cache_root).unwrap();
    let path = cache_root.join("owner.sqlite3");
    let device = RetainedEnrollmentClient::new(
        state.clone(),
        path.clone(),
        Arc::new(Wall),
        Arc::new(Time(Instant::now())),
    )
    .unwrap();
    let status = runtime
        .block_on(device.status(TcpStream::connect(address).unwrap(), None))
        .unwrap();
    let NativePairingStatus::Active {
        receiver,
        access_epoch,
        ..
    } = status.enrollment().as_ref().unwrap()
    else {
        panic!("paired device must be active")
    };
    let saved = state.load_credential().unwrap().unwrap();
    let credential = saved.credential().as_str().to_owned();
    let (key, pin, _) = saved.into_enrollment().into_parts();
    let identity = NativeIdentity::restore(key).unwrap();
    let mut cache =
        RetainedCache::open(&path, Arc::new(Wall), Arc::new(Time(Instant::now()))).unwrap();
    let access = || ReaderAccess {
        role: ReaderRole::DeviceOwner,
        address,
        relay: None,
        identity: &identity,
        pin,
        credential: &credential,
        receiver: receiver.as_str(),
        access_epoch: *access_epoch,
        client_id: "retained-library-test",
    };
    cache
        .read(
            access(),
            &ReadStop::new(),
            Duration::from_secs(5),
            8,
            &mut Vec::new(),
        )
        .unwrap();
    let scope = cache
        .saved_catalogue_scope(receiver.as_str())
        .unwrap()
        .unwrap();
    let conversation = ConversationId::new(&setup.conversation).unwrap();
    let RetainedTranscript::Retained(snapshot) = cache
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
    let retained = serde_json::to_value(snapshot.view()).unwrap();
    let mut feed = cache
        .subscribe(
            access(),
            &conversation,
            None,
            &ReadStop::new(),
            Duration::from_secs(5),
        )
        .unwrap();
    let mut live = None;
    for _ in 0..8 {
        match feed.next_event().unwrap() {
            FeedEvent::View { cursor, view } => {
                let value = serde_json::to_value(&view).unwrap();
                let stale = value["transcriptState"] == "stale";
                live = Some((cursor.incarnation, cursor.position, value));
                if !stale {
                    break;
                }
            }
            FeedEvent::Ended { reason, code, .. } => {
                panic!("subscription ended before a settled view: {reason} {code:?}")
            }
        }
    }
    let (incarnation, position, view) = live.expect("the feed delivered a view");
    assert_eq!(activity(&view), activity(&retained));
    feed.close().unwrap();
    let mut resumed = cache
        .subscribe(
            access(),
            &conversation,
            Some(ConversationViewCursor {
                incarnation: incarnation.clone(),
                position: position.clone(),
            }),
            &ReadStop::new(),
            Duration::from_secs(5),
        )
        .unwrap();
    match resumed.next_event().unwrap() {
        FeedEvent::View { cursor, .. } => {
            assert_eq!(cursor.incarnation, incarnation);
            let resumed_at: u64 = cursor.position.parse().unwrap();
            let applied: u64 = position.parse().unwrap();
            assert!(resumed_at >= applied);
        }
        FeedEvent::Ended { reason, code, .. } => {
            panic!("resubscribe ended before a view: {reason} {code:?}")
        }
    }
    resumed.close().unwrap();
    let missing = ConversationId::new("00000000-0000-4000-8000-000000000099").unwrap();
    let refused = match cache.subscribe(
        access(),
        &missing,
        None,
        &ReadStop::new(),
        Duration::from_secs(5),
    ) {
        Err(error) => error,
        Ok(feed) => {
            drop(feed);
            panic!("an ungranted conversation must be refused");
        }
    };
    assert_eq!(refused.failure(), ReadFailure::Refused);
    assert!(!refused.asks_status());
    drop(cache);
    drop(gateway);
    runtime.block_on(device.shutdown());
}

/// Activity the retained records fold and the subscription view both own. The live
/// overlay (title, capabilities, approval mode, revision, gateway time) is
/// not part of the retained records.
fn activity(view: &Value) -> Value {
    let field = |value: &Value, name: &str| value.get(name).cloned().unwrap_or(Value::Null);
    let messages = view["messages"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|message| {
            serde_json::json!({
                "executionId": field(message, "executionId"),
                "userText": field(message, "userText"),
                "status": field(message, "status"),
            })
        })
        .collect::<Vec<_>>();
    let pending = view["pending"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|message| {
            serde_json::json!({
                "executionId": field(message, "executionId"),
                "text": field(message, "text"),
            })
        })
        .collect::<Vec<_>>();
    let notes = view["notes"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|note| {
            serde_json::json!({
                "messageId": field(note, "messageId"),
                "text": field(note, "text"),
            })
        })
        .collect::<Vec<_>>();
    let artifacts = view["artifacts"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|artifact| {
            serde_json::json!({
                "name": field(artifact, "name"),
                "digest": field(artifact, "digest"),
                "mimeType": field(artifact, "mimeType"),
                "size": field(artifact, "size"),
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "messages": messages,
        "pending": pending,
        "notes": notes,
        "artifacts": artifacts,
        "transcriptState": field(view, "transcriptState"),
    })
}
