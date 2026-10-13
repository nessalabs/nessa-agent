//! The export lease: admitted work is in the cut, later work waits, and a
//! failure or retirement publishes nothing.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::{OwnedRwLockWriteGuard, RwLock};

use crate::backup::application::{
    capture_export, AdmissionFailure, AdmissionLease, Authenticator, DeletionLogStore, ExportClock,
    ExportFailure, ExportRequest, ExportSource, ManifestPublisher, MutationAdmission,
    PublishedRecord,
};
use crate::backup::domain::{
    AuthenticatedManifest, CapturedRecord, DeletionLog, Fingerprint, Sensitivity, SkipReason,
    SkippedRecord, SourceCut, StoreKind, Watermark, WRITER_INVENTORY,
};

#[tokio::test]
async fn an_admitted_mutation_is_in_the_cut() {
    let lock = Arc::new(RwLock::new(()));
    let value = Arc::new(AtomicU64::new(0));
    let started = Arc::new(AtomicBool::new(false));
    let permit = lock.read().await;
    value.store(1, Ordering::SeqCst);
    let seen = value.clone();
    let started_for_source = started.clone();
    let admission = LockAdmission::new(lock.clone());
    let publisher = MemoryPublisher::default();
    let sources = sources_with(StoreKind::ConversationMetadata, move || {
        started_for_source.store(true, Ordering::SeqCst);
        with_body(
            StoreKind::ConversationMetadata,
            seen.load(Ordering::SeqCst).to_string().into_bytes(),
        )
    });
    let capture = tokio::spawn(run(sources, admission, publisher.clone()));
    tokio::task::yield_now().await;
    assert!(
        !started.load(Ordering::SeqCst),
        "the cut waits for the admitted write"
    );
    drop(permit);
    let manifest = capture.await.unwrap().unwrap();
    assert_eq!(
        manifest
            .manifest()
            .store(StoreKind::ConversationMetadata)
            .records()[0]
            .public_bytes(),
        Some(b"1".as_slice())
    );
}

#[tokio::test]
async fn a_mutation_during_the_hold_waits_until_release() {
    let lock = Arc::new(RwLock::new(()));
    let value = Arc::new(AtomicU64::new(1));
    let inside = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let admission = LockAdmission::new(lock.clone());
    let publisher = MemoryPublisher::default();
    let sources = sources_replacing(
        StoreKind::ConversationMetadata,
        Box::new(Gated {
            kind: StoreKind::ConversationMetadata,
            value: value.clone(),
            inside: inside.clone(),
            release: release.clone(),
        }),
    );
    let capture = tokio::spawn(run(sources, admission, publisher));
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while !inside.load(Ordering::SeqCst) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the cut did not reach its read"
        );
        tokio::task::yield_now().await;
    }
    let value_for_mutation = value.clone();
    let lock_for_mutation = lock.clone();
    let mutation = tokio::spawn(async move {
        let _permit = lock_for_mutation.read().await;
        value_for_mutation.store(2, Ordering::SeqCst);
    });
    tokio::task::yield_now().await;
    assert_eq!(
        value.load(Ordering::SeqCst),
        1,
        "the mutation waits out the hold"
    );
    release.store(true, Ordering::SeqCst);
    let manifest = capture.await.unwrap().unwrap();
    assert_eq!(
        manifest
            .manifest()
            .store(StoreKind::ConversationMetadata)
            .records()[0]
            .public_bytes(),
        Some(b"1".as_slice())
    );
    mutation.await.unwrap();
    assert_eq!(value.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_deletion_during_the_hold_is_entirely_in_or_entirely_out() {
    let lock = Arc::new(RwLock::new(()));
    let present = Arc::new(AtomicBool::new(true));
    let permit = lock.read().await;
    present.store(false, Ordering::SeqCst);
    let seen = present.clone();
    let admission = LockAdmission::new(lock.clone());
    let publisher = MemoryPublisher::default();
    let sources = sources_with(StoreKind::ConversationMetadata, move || {
        with_body(
            StoreKind::ConversationMetadata,
            if seen.load(Ordering::SeqCst) {
                b"present".to_vec()
            } else {
                b"fenced".to_vec()
            },
        )
    });
    let capture = tokio::spawn(run(sources, admission, publisher));
    tokio::task::yield_now().await;
    drop(permit);
    let included = capture.await.unwrap().unwrap();
    assert_eq!(
        included
            .manifest()
            .store(StoreKind::ConversationMetadata)
            .records()[0]
            .public_bytes(),
        Some(b"fenced".as_slice()),
        "a deletion that held admission is entirely in the cut"
    );

    let present = Arc::new(AtomicBool::new(true));
    let seen = present.clone();
    let admission = LockAdmission::new(lock.clone());
    let publisher = MemoryPublisher::default();
    let hold = admission.hold().await.unwrap();
    let sources = sources_with(StoreKind::ConversationMetadata, move || {
        with_body(
            StoreKind::ConversationMetadata,
            if seen.load(Ordering::SeqCst) {
                b"present".to_vec()
            } else {
                b"fenced".to_vec()
            },
        )
    });
    let capture = tokio::spawn(run(sources, admission, publisher));
    let present_for_deletion = present.clone();
    let deletion = tokio::spawn(async move {
        let _permit = lock.read().await;
        present_for_deletion.store(false, Ordering::SeqCst);
    });
    tokio::task::yield_now().await;
    assert!(present.load(Ordering::SeqCst));
    drop(hold);
    let excluded = capture.await.unwrap().unwrap();
    assert_eq!(
        excluded
            .manifest()
            .store(StoreKind::ConversationMetadata)
            .records()[0]
            .public_bytes(),
        Some(b"present".as_slice()),
        "a deletion that waited is entirely out of the cut"
    );
    deletion.await.unwrap();
    assert!(!present.load(Ordering::SeqCst));
}

#[tokio::test]
async fn an_unreadable_record_is_skipped_and_named() {
    let publisher = MemoryPublisher::default();
    let manifest = run(
        sources_with(StoreKind::LeaseRecords, || {
            let skipped =
                SkippedRecord::new(StoreKind::LeaseRecords, "line-2", SkipReason::Unreadable)
                    .unwrap();
            let record = CapturedRecord::new(
                StoreKind::LeaseRecords,
                "line-1",
                Sensitivity::Public,
                br#"{"lease":1}"#.to_vec(),
            )
            .unwrap();
            SourceCut::new(
                StoreKind::LeaseRecords,
                Watermark::new("2").unwrap(),
                vec![record],
                vec![skipped],
            )
            .unwrap()
        }),
        LockAdmission::new(Arc::new(RwLock::new(()))),
        publisher,
    )
    .await
    .unwrap();
    let cut = manifest.manifest().store(StoreKind::LeaseRecords);
    assert_eq!(cut.skipped()[0].identity(), "line-2");
    assert!(manifest
        .encoded()
        .contains("skipped lease-records line-2 unreadable"));
}

#[tokio::test]
async fn an_unavailable_store_is_skipped_and_the_cut_continues() {
    let publisher = MemoryPublisher::default();
    let manifest = run(
        sources_with_error(StoreKind::DriveToolAudit),
        LockAdmission::new(Arc::new(RwLock::new(()))),
        publisher.clone(),
    )
    .await
    .unwrap();
    let drive = manifest.manifest().store(StoreKind::DriveToolAudit);
    assert_eq!(drive.skipped()[0].identity(), "store");
    assert_eq!(drive.skipped()[0].reason(), SkipReason::Unavailable);
    assert!(!publisher.manifest.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_secret_is_fingerprinted_and_left_out_of_the_bundle() {
    let secret = b"plain-text-secret".to_vec();
    let publisher = MemoryPublisher::default();
    let manifest = run(
        sources_with(StoreKind::DeviceCredentials, || {
            let record = CapturedRecord::new(
                StoreKind::DeviceCredentials,
                "gateway-key",
                Sensitivity::Secret,
                b"plain-text-secret".to_vec(),
            )
            .unwrap();
            SourceCut::new(
                StoreKind::DeviceCredentials,
                Watermark::new("1").unwrap(),
                vec![record],
                vec![],
            )
            .unwrap()
        }),
        LockAdmission::new(Arc::new(RwLock::new(()))),
        publisher.clone(),
    )
    .await
    .unwrap();
    let record = &manifest
        .manifest()
        .store(StoreKind::DeviceCredentials)
        .records()[0];
    assert!(record.public_bytes().is_none());
    assert_eq!(record.fingerprint(), Fingerprint::of(&secret));
    assert!(publisher.records.lock().unwrap().is_empty());
    assert!(!manifest.encoded().contains("plain-text-secret"));
    assert!(manifest.encoded().contains("redacted device-credentials"));
}

#[tokio::test]
async fn a_failed_export_releases_admission_and_publishes_nothing() {
    let lock = Arc::new(RwLock::new(()));
    let released = Arc::new(AtomicBool::new(false));
    let publisher = MemoryPublisher {
        fail: true,
        ..MemoryPublisher::default()
    };
    let outcome = run(
        empty_sources(),
        LockAdmission {
            lock: lock.clone(),
            retired: Arc::new(AtomicBool::new(false)),
            holds: Arc::new(AtomicU64::new(0)),
            released: released.clone(),
        },
        publisher.clone(),
    )
    .await;
    assert_eq!(outcome, Err(ExportFailure::Publish));
    assert!(released.load(Ordering::SeqCst));
    assert!(publisher.manifest.lock().unwrap().is_empty());
    let _permit = lock.read().await;
}

#[tokio::test]
async fn retirement_before_the_hold_aborts() {
    let publisher = MemoryPublisher::default();
    let holds = Arc::new(AtomicU64::new(0));
    let outcome = run(
        empty_sources(),
        LockAdmission {
            lock: Arc::new(RwLock::new(())),
            retired: Arc::new(AtomicBool::new(true)),
            holds: holds.clone(),
            released: Arc::new(AtomicBool::new(false)),
        },
        publisher.clone(),
    )
    .await;
    assert_eq!(outcome, Err(ExportFailure::Retired));
    assert_eq!(holds.load(Ordering::SeqCst), 0);
    assert!(publisher.manifest.lock().unwrap().is_empty());
}

#[tokio::test]
async fn retirement_during_the_hold_aborts_and_releases() {
    let publisher = MemoryPublisher::default();
    let released = Arc::new(AtomicBool::new(false));
    let outcome = run(
        empty_sources(),
        FlipAdmission {
            released: released.clone(),
        },
        publisher.clone(),
    )
    .await;
    assert_eq!(outcome, Err(ExportFailure::Retired));
    assert!(released.load(Ordering::SeqCst));
    assert!(publisher.manifest.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_cut_refuses_to_start_without_every_store() {
    let holds = Arc::new(AtomicU64::new(0));
    let admission = LockAdmission {
        lock: Arc::new(RwLock::new(())),
        retired: Arc::new(AtomicBool::new(false)),
        holds: holds.clone(),
        released: Arc::new(AtomicBool::new(false)),
    };
    let publisher = MemoryPublisher::default();
    let sources: Vec<Box<dyn ExportSource>> = Vec::new();
    let outcome = run(sources, admission, publisher).await;
    assert_eq!(outcome, Err(ExportFailure::Inventory));
    assert_eq!(holds.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_authenticator_failure_publishes_nothing() {
    let publisher = MemoryPublisher::default();
    let released = Arc::new(AtomicBool::new(false));
    let admission = LockAdmission {
        lock: Arc::new(RwLock::new(())),
        retired: Arc::new(AtomicBool::new(false)),
        holds: Arc::new(AtomicU64::new(0)),
        released: released.clone(),
    };
    let sources = empty_sources();
    let refs: Vec<&dyn ExportSource> = sources.iter().map(|source| source.as_ref()).collect();
    let clock = Clock(4);
    let outcome = capture_export(ExportRequest {
        admission: &admission,
        sources: &refs,
        fences: &[],
        inventory: &EmptyLog,
        publisher: &publisher,
        clock: &clock,
        authenticator: &FailMac,
    })
    .await;
    assert_eq!(outcome, Err(ExportFailure::Authenticator));
    assert!(released.load(Ordering::SeqCst));
    assert!(publisher.manifest.lock().unwrap().is_empty());
}

async fn run(
    sources: Vec<Box<dyn ExportSource>>,
    admission: impl MutationAdmission,
    publisher: MemoryPublisher,
) -> Result<AuthenticatedManifest, ExportFailure> {
    let refs: Vec<&dyn ExportSource> = sources.iter().map(|source| source.as_ref()).collect();
    let clock = Clock(4);
    let mac = crate::backup::application::MacKey([3; 32]);
    capture_export(ExportRequest {
        admission: &admission,
        sources: &refs,
        fences: &[],
        inventory: &EmptyLog,
        publisher: &publisher,
        clock: &clock,
        authenticator: &mac,
    })
    .await
}

fn empty_sources() -> Vec<Box<dyn ExportSource>> {
    WRITER_INVENTORY
        .iter()
        .copied()
        .map(|kind| Box::new(Fixed::ready(empty(kind))) as Box<dyn ExportSource>)
        .collect()
}

fn sources_with(
    kind: StoreKind,
    cut: impl Fn() -> SourceCut + Send + Sync + 'static,
) -> Vec<Box<dyn ExportSource>> {
    let cut = Arc::new(cut);
    WRITER_INVENTORY
        .iter()
        .copied()
        .map(|stored| {
            if stored == kind {
                let cut = cut.clone();
                Box::new(Fixed::from_fn(kind, cut)) as Box<dyn ExportSource>
            } else {
                Box::new(Fixed::ready(empty(stored))) as Box<dyn ExportSource>
            }
        })
        .collect()
}

fn sources_replacing(
    kind: StoreKind,
    replacement: Box<dyn ExportSource>,
) -> Vec<Box<dyn ExportSource>> {
    let mut replacement = Some(replacement);
    WRITER_INVENTORY
        .iter()
        .copied()
        .map(|stored| {
            if stored == kind {
                replacement.take().expect("one store is replaced")
            } else {
                Box::new(Fixed::ready(empty(stored))) as Box<dyn ExportSource>
            }
        })
        .collect()
}

struct Gated {
    kind: StoreKind,
    value: Arc<AtomicU64>,
    inside: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
}

#[async_trait]
impl ExportSource for Gated {
    fn kind(&self) -> StoreKind {
        self.kind
    }

    async fn capture(&self) -> Result<SourceCut, ()> {
        let body = self.value.load(Ordering::SeqCst).to_string().into_bytes();
        self.inside.store(true, Ordering::SeqCst);
        while !self.release.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
        Ok(with_body(self.kind, body))
    }
}

fn sources_with_error(kind: StoreKind) -> Vec<Box<dyn ExportSource>> {
    WRITER_INVENTORY
        .iter()
        .copied()
        .map(|stored| {
            if stored == kind {
                Box::new(Fixed::fail(kind)) as Box<dyn ExportSource>
            } else {
                Box::new(Fixed::ready(empty(stored))) as Box<dyn ExportSource>
            }
        })
        .collect()
}

fn empty(kind: StoreKind) -> SourceCut {
    SourceCut::new(kind, Watermark::new("absent").unwrap(), vec![], vec![]).unwrap()
}

fn with_body(kind: StoreKind, body: Vec<u8>) -> SourceCut {
    let record = CapturedRecord::new(kind, "row", Sensitivity::Public, body).unwrap();
    SourceCut::new(kind, Watermark::new("1").unwrap(), vec![record], vec![]).unwrap()
}

struct LockAdmission {
    lock: Arc<RwLock<()>>,
    retired: Arc<AtomicBool>,
    holds: Arc<AtomicU64>,
    released: Arc<AtomicBool>,
}

impl LockAdmission {
    fn new(lock: Arc<RwLock<()>>) -> Self {
        Self {
            lock,
            retired: Arc::new(AtomicBool::new(false)),
            holds: Arc::new(AtomicU64::new(0)),
            released: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[async_trait]
impl MutationAdmission for LockAdmission {
    async fn hold(&self) -> Result<Box<dyn AdmissionLease + Send + Sync>, AdmissionFailure> {
        if self.retired.load(Ordering::SeqCst) {
            return Err(AdmissionFailure::Retired);
        }
        self.holds.fetch_add(1, Ordering::SeqCst);
        let guard = self.lock.clone().write_owned().await;
        if self.retired.load(Ordering::SeqCst) {
            return Err(AdmissionFailure::Retired);
        }
        Ok(Box::new(LockHold {
            _guard: guard,
            released: self.released.clone(),
        }))
    }
}

struct LockHold {
    _guard: OwnedRwLockWriteGuard<()>,
    released: Arc<AtomicBool>,
}

impl Drop for LockHold {
    fn drop(&mut self) {
        self.released.store(true, Ordering::SeqCst);
    }
}

impl AdmissionLease for LockHold {
    fn retirement_signaled(&self) -> bool {
        false
    }
}

struct FlipAdmission {
    released: Arc<AtomicBool>,
}

#[async_trait]
impl MutationAdmission for FlipAdmission {
    async fn hold(&self) -> Result<Box<dyn AdmissionLease + Send + Sync>, AdmissionFailure> {
        Ok(Box::new(FlipHold {
            checks: AtomicU64::new(0),
            released: self.released.clone(),
        }))
    }
}

struct FlipHold {
    checks: AtomicU64,
    released: Arc<AtomicBool>,
}

impl Drop for FlipHold {
    fn drop(&mut self) {
        self.released.store(true, Ordering::SeqCst);
    }
}

impl AdmissionLease for FlipHold {
    fn retirement_signaled(&self) -> bool {
        self.checks.fetch_add(1, Ordering::SeqCst) >= 1
    }
}

struct Fixed {
    kind: StoreKind,
    cut: Mutex<Option<SourceCut>>,
    fail: bool,
    build: Option<Arc<dyn Fn() -> SourceCut + Send + Sync>>,
}

impl Fixed {
    fn ready(cut: SourceCut) -> Self {
        Self {
            kind: cut.store(),
            cut: Mutex::new(Some(cut)),
            fail: false,
            build: None,
        }
    }

    fn fail(kind: StoreKind) -> Self {
        Self {
            kind,
            cut: Mutex::new(None),
            fail: true,
            build: None,
        }
    }

    fn from_fn(kind: StoreKind, build: Arc<dyn Fn() -> SourceCut + Send + Sync>) -> Self {
        Self {
            kind,
            cut: Mutex::new(None),
            fail: false,
            build: Some(build),
        }
    }
}

#[async_trait]
impl ExportSource for Fixed {
    fn kind(&self) -> StoreKind {
        self.kind
    }

    async fn capture(&self) -> Result<SourceCut, ()> {
        if self.fail {
            return Err(());
        }
        if let Some(build) = &self.build {
            return Ok(build());
        }
        self.cut.lock().unwrap().take().ok_or(())
    }
}

struct EmptyLog;

#[async_trait]
impl DeletionLogStore for EmptyLog {
    async fn load(&self) -> Result<DeletionLog, ()> {
        Ok(DeletionLog::default())
    }

    async fn store(&self, _next: &DeletionLog) -> Result<(), ()> {
        Ok(())
    }
}

#[derive(Clone, Default)]
struct MemoryPublisher {
    fail: bool,
    manifest: Arc<Mutex<String>>,
    records: Arc<Mutex<Vec<PublishedRecord>>>,
}

#[async_trait]
impl ManifestPublisher for MemoryPublisher {
    async fn put(&self, manifest: &str, records: &[PublishedRecord]) -> Result<(), ()> {
        if self.fail {
            return Err(());
        }
        *self.manifest.lock().unwrap() = manifest.to_owned();
        *self.records.lock().unwrap() = records.to_vec();
        Ok(())
    }
}

struct FailMac;

impl Authenticator for FailMac {
    fn mac(&self, _canonical: &str) -> Result<Fingerprint, ()> {
        Err(())
    }
}

struct Clock(u64);

impl ExportClock for Clock {
    fn unix_ms(&self) -> u64 {
        self.0
    }
}
