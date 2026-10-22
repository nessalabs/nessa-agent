//! Quarantine, acceptance, and on-demand fetch of a deferred conversation.
//!
//! A failed or partial stage leaves the live path untouched. Acceptance is a
//! rename onto an empty live path. An oversized conversation is fetched only
//! when it is opened, and only when its stream still matches the cut.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;

use crate::backup::application::{
    accept_quarantine, current_staging_id, open_deferred_conversation, ordered_roots, root_posture,
    stage_quarantine, AcceptRequest, AdmissionFailure, AdmissionLease, AnchoredRoot, Authenticator,
    DecisionAuditError, DeferredSessionSource, ExportClock, FetchProgress, InventoryCause,
    InventoryDecision, JournalFields, LeaseReceipts, LoadedTree, MacKey, ManifestPublisher,
    MutationAdmission, OpenReport, OpenRequest, Promoted, PublicationIds, PublishedRecord,
    PublishedSession, QuarantineLayout, RestoreAction, RestoreAudit, RestoreEvidence, RestoreFacts,
    RestoreHold, RestorePolicyEvaluator, SealFields, SessionAttempt, SessionCut,
    SessionSourceError, StageReport, StageRequest, StagedTreeOwner, Staging, StagingIds,
    VerificationFailure, VerificationLimits, VerificationSource,
};
use crate::backup::domain::{
    encode_journal, read_journal, seal_staging_id, AuthenticatedManifest, CapturedRecord,
    CredentialKind, DeferredEntry, DeletionFence, DeletionLog, Fingerprint, Freshness, GrantRole,
    Manifest, Posture, RestoreFault, Sensitivity, SourceCut, StoreKind, VolumeCase, Watermark,
    WRITER_INVENTORY,
};
use crate::backup::infrastructure::snapshot::SESSION_EXPORT_LIMIT;
use crate::backup::infrastructure::{
    capture_namespace, CedarInventoryPolicy, DirectoryMaterializer, DirectoryPublisher,
    DirectoryQuarantine, DirectoryVerificationSource, ExportFormat, FileDeletionLog,
    FileRestoreAudit, KeyedInventory, OsPublicationIds, OsStagingIds, SqliteDeferredSource,
};

struct Roots {
    _keep: tempfile::TempDir,
    quarantine: PathBuf,
    live: PathBuf,
}

fn roots() -> Roots {
    let keep = tempfile::tempdir().unwrap();
    Roots {
        quarantine: keep.path().join("quarantine"),
        live: keep.path().join("live"),
        _keep: keep,
    }
}

fn person(action: RestoreAction) -> RestoreFacts<'static> {
    RestoreFacts {
        kind: "person",
        cause: InventoryCause::Operator,
        action,
    }
}

fn policy() -> CedarInventoryPolicy {
    CedarInventoryPolicy::new().expect("the restore policies validate")
}

struct Quiet;

impl FetchProgress for Quiet {
    fn report(&mut self, _read: u64, _total: u64) {}
}

struct NoReceipts;

impl LeaseReceipts for NoReceipts {
    fn receipt(&self, _lease_id: &str) -> Option<String> {
        None
    }
}

struct MapReceipts<'a>(&'a [(&'a str, &'a str)]);

impl LeaseReceipts for MapReceipts<'_> {
    fn receipt(&self, lease_id: &str) -> Option<String> {
        self.0
            .iter()
            .find(|(id, _)| *id == lease_id)
            .map(|(_, text)| (*text).to_owned())
    }
}

#[derive(Clone, Debug)]
struct AuditLine {
    target: String,
    before: String,
    after: String,
    cause: String,
    initiator: String,
    decision: String,
    policies: Vec<String>,
    attestations: String,
    journal_digest: String,
    root_generation: String,
}

struct MemoryAudit {
    fail_at: Option<usize>,
    lines: Mutex<Vec<AuditLine>>,
    history: Vec<AuditLine>,
    trust_unavailable: bool,
}

impl MemoryAudit {
    fn open() -> Self {
        Self {
            fail_at: None,
            lines: Mutex::new(Vec::new()),
            history: Vec::new(),
            trust_unavailable: false,
        }
    }

    fn fail_after(count: usize) -> Self {
        Self {
            fail_at: Some(count),
            lines: Mutex::new(Vec::new()),
            history: Vec::new(),
            trust_unavailable: false,
        }
    }

    /// Start a new assertion/failure-injection window while retaining actual
    /// stage/accept records as the journal authority for later opens.
    fn checkpoint(&self) -> Self {
        let mut history = self.history.clone();
        history.extend(self.lines());
        Self {
            fail_at: None,
            lines: Mutex::new(Vec::new()),
            history,
            trust_unavailable: false,
        }
    }

    fn failing_after(&self, count: usize) -> Self {
        Self {
            fail_at: Some(count),
            ..self.checkpoint()
        }
    }

    fn lines(&self) -> Vec<AuditLine> {
        self.lines.lock().expect("audit").clone()
    }
}

impl RestoreAudit for MemoryAudit {
    fn append(&self, evidence: &RestoreEvidence<'_>) -> Result<(), DecisionAuditError> {
        let mut lines = self
            .lines
            .lock()
            .map_err(|_| DecisionAuditError::NotStored)?;
        if self.fail_at.is_some_and(|count| lines.len() >= count) {
            return Err(DecisionAuditError::NotStored);
        }
        lines.push(AuditLine {
            target: evidence.target.to_owned(),
            before: evidence.before.to_owned(),
            after: evidence.after.to_owned(),
            cause: evidence.cause.to_owned(),
            initiator: evidence.initiator.to_owned(),
            decision: evidence.decision.to_owned(),
            policies: evidence.policies.to_vec(),
            attestations: evidence.attestations.to_owned(),
            journal_digest: evidence.journal_digest.to_owned(),
            root_generation: evidence.root_generation.to_owned(),
        });
        Ok(())
    }

    fn available(
        &self,
        conversation_id: &str,
        publication: &str,
    ) -> Result<bool, DecisionAuditError> {
        let lines = self
            .lines
            .lock()
            .map_err(|_| DecisionAuditError::NotStored)?;
        Ok(self.history.iter().chain(lines.iter()).any(|line| {
            line.after == "available"
                && line.decision == "effect"
                && line.attestations == publication
                && crate::backup::application::fetch_names_conversation(
                    &line.target,
                    conversation_id,
                )
        }))
    }

    fn sealed(&self, target: &str, digest: &str) -> Result<Option<String>, DecisionAuditError> {
        let lines = self
            .lines
            .lock()
            .map_err(|_| DecisionAuditError::NotStored)?;
        Ok(current_staging_id(
            self.history
                .iter()
                .chain(lines.iter())
                .map(|line| JournalFields {
                    seal: SealFields {
                        target: &line.target,
                        before: &line.before,
                        after: &line.after,
                        cause: &line.cause,
                        decision: &line.decision,
                        attestations: &line.attestations,
                    },
                    journal_digest: &line.journal_digest,
                    root_generation: &line.root_generation,
                }),
            target,
            digest,
        ))
    }

    fn trusted_journal(
        &self,
        target: &str,
        digest: &str,
        staging_id: Option<&str>,
    ) -> Result<bool, DecisionAuditError> {
        if self.trust_unavailable {
            return Err(DecisionAuditError::NotStored);
        }
        let lines = self
            .lines
            .lock()
            .map_err(|_| DecisionAuditError::NotStored)?;
        Ok(crate::backup::application::current_journal_is_trusted(
            self.history
                .iter()
                .chain(lines.iter())
                .map(|line| JournalFields {
                    seal: SealFields {
                        target: &line.target,
                        before: &line.before,
                        after: &line.after,
                        cause: &line.cause,
                        decision: &line.decision,
                        attestations: &line.attestations,
                    },
                    journal_digest: &line.journal_digest,
                    root_generation: &line.root_generation,
                }),
            target,
            digest,
            staging_id,
        ))
    }
}

struct Clock(u64);

impl ExportClock for Clock {
    fn unix_ms(&self) -> u64 {
        self.0
    }
}

struct OpenAdmission;

#[async_trait]
impl MutationAdmission for OpenAdmission {
    async fn hold(&self) -> Result<Box<dyn AdmissionLease + Send + Sync>, AdmissionFailure> {
        Ok(Box::new(OpenHold))
    }
}

struct OpenHold;

impl AdmissionLease for OpenHold {
    fn retirement_signaled(&self) -> bool {
        false
    }
}

enum Bytes {
    Public(Vec<u8>),
    Secret(Vec<u8>),
}

struct Row {
    store: StoreKind,
    identity: String,
    bytes: Bytes,
}

struct Bundle {
    _keep: tempfile::TempDir,
    root: PathBuf,
    key: MacKey,
}

async fn publish(
    rows: Vec<Row>,
    deferred: Vec<DeferredEntry>,
    inventory: Option<DeletionLog>,
) -> Bundle {
    let key = MacKey([7; 32]);
    let mut captured: Vec<(StoreKind, Vec<CapturedRecord>)> = Vec::new();
    let mut published = Vec::new();
    for row in rows {
        let record = match &row.bytes {
            Bytes::Public(bytes) => {
                published.push(PublishedRecord {
                    store: row.store,
                    identity: row.identity.clone(),
                    bytes: bytes.clone(),
                });
                CapturedRecord::new(row.store, row.identity, Sensitivity::Public, bytes.clone())
            }
            Bytes::Secret(bytes) => {
                CapturedRecord::new(row.store, row.identity, Sensitivity::Secret, bytes.clone())
            }
        }
        .unwrap();
        if let Some((_, existing)) = captured.iter_mut().find(|(store, _)| *store == row.store) {
            existing.push(record);
        } else {
            captured.push((row.store, vec![record]));
        }
    }
    if let Some(log) = &inventory {
        let bytes = log.canonical().into_bytes();
        published.push(PublishedRecord {
            store: StoreKind::DeletionInventory,
            identity: "log".into(),
            bytes: bytes.clone(),
        });
        let record = CapturedRecord::new(
            StoreKind::DeletionInventory,
            "log",
            Sensitivity::Public,
            bytes,
        )
        .unwrap();
        captured.push((StoreKind::DeletionInventory, vec![record]));
    }
    let cuts = WRITER_INVENTORY
        .iter()
        .map(|store| {
            if *store == StoreKind::DeletionInventory {
                if let Some(log) = &inventory {
                    let record = CapturedRecord::new(
                        *store,
                        "log",
                        Sensitivity::Public,
                        log.canonical().into_bytes(),
                    )
                    .unwrap();
                    return SourceCut::new(
                        *store,
                        Watermark::new(log.watermark()).unwrap(),
                        vec![record],
                        vec![],
                    )
                    .unwrap();
                }
            }
            let records = captured
                .iter()
                .find(|(kind, _)| kind == store)
                .map(|(_, records)| records.clone())
                .unwrap_or_default();
            let deferred = deferred
                .iter()
                .filter(|entry| entry.store() == *store)
                .cloned()
                .collect::<Vec<_>>();
            if records.is_empty() && deferred.is_empty() {
                SourceCut::absent(*store).unwrap()
            } else {
                SourceCut::with_deferred(
                    *store,
                    Watermark::new("cut").unwrap(),
                    records,
                    vec![],
                    deferred,
                )
                .unwrap()
            }
        })
        .collect();
    let manifest = Manifest::new(8, cuts).unwrap();
    let mac = key.mac(&manifest.canonical()).unwrap();
    let encoded = AuthenticatedManifest::seal(manifest, mac).encoded();
    let keep = tempfile::tempdir().unwrap();
    let root = keep.path().join("bundle");
    DirectoryPublisher::new(&root)
        .put(&encoded, &published)
        .await
        .unwrap();
    Bundle {
        _keep: keep,
        root,
        key,
    }
}

fn seal_inventory(key: MacKey, log: &DeletionLog) -> String {
    let canonical = log.canonical();
    let mac = key.mac(&canonical).unwrap();
    format!("{canonical}mac {}\n", mac.hex())
}

fn stage(
    bundle: &Bundle,
    quarantine: &Path,
    live: &Path,
    audit: &dyn RestoreAudit,
    facts: RestoreFacts<'_>,
    later: Option<&str>,
) -> Result<StageReport, RestoreFault> {
    let source = DirectoryVerificationSource::open(bundle.root.clone()).unwrap();
    stage_quarantine(
        &StageRequest {
            facts,
            bundle: &source,
            format: &ExportFormat,
            verifier: &bundle.key,
            limits: VerificationLimits::default(),
            later_inventory: later,
            quarantine,
            live,
        },
        &policy(),
        audit,
        &DirectoryQuarantine,
        &DirectoryMaterializer,
        &KeyedInventory::new(bundle.key),
        &OsStagingIds,
    )
}

struct SecondManifest {
    inner: DirectoryVerificationSource,
    reads: AtomicUsize,
}

impl VerificationSource for SecondManifest {
    fn manifest(&self) -> Result<Box<dyn Read + '_>, VerificationFailure> {
        let read = self.reads.fetch_add(1, Ordering::SeqCst);
        if read == 0 {
            self.inner.manifest()
        } else {
            Ok(Box::new(std::io::Cursor::new(
                b"unauthenticated-manifest".to_vec(),
            )))
        }
    }

    fn record(
        &self,
        store: StoreKind,
        encoded_identity: &str,
    ) -> Result<Box<dyn Read + '_>, VerificationFailure> {
        self.inner.record(store, encoded_identity)
    }

    fn visit_records(
        &self,
        visitor: &mut dyn FnMut(StoreKind, &str) -> Result<(), VerificationFailure>,
    ) -> Result<(), VerificationFailure> {
        self.inner.visit_records(visitor)
    }
}

#[tokio::test]
async fn a_replaced_manifest_is_not_what_quarantine_seals() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    let source = SecondManifest {
        inner: DirectoryVerificationSource::open(bundle.root.clone()).unwrap(),
        reads: AtomicUsize::new(0),
    };
    stage_quarantine(
        &StageRequest {
            facts: person(RestoreAction::Stage),
            bundle: &source,
            format: &ExportFormat,
            verifier: &bundle.key,
            limits: VerificationLimits::default(),
            later_inventory: None,
            quarantine: &place.quarantine,
            live: &place.live,
        },
        &policy(),
        &MemoryAudit::open(),
        &DirectoryQuarantine,
        &DirectoryMaterializer,
        &KeyedInventory::new(bundle.key),
        &OsStagingIds,
    )
    .unwrap();
    assert_eq!(source.reads.load(Ordering::SeqCst), 1);
    assert_eq!(
        fs::read(place.quarantine.join("records/notes/note-1")).unwrap(),
        b"a public note"
    );
}

struct SecondInventory {
    inner: DirectoryVerificationSource,
    reads: AtomicUsize,
    replacement: Vec<u8>,
}

impl VerificationSource for SecondInventory {
    fn manifest(&self) -> Result<Box<dyn Read + '_>, VerificationFailure> {
        self.inner.manifest()
    }

    fn record(
        &self,
        store: StoreKind,
        encoded_identity: &str,
    ) -> Result<Box<dyn Read + '_>, VerificationFailure> {
        if store == StoreKind::DeletionInventory {
            let read = self.reads.fetch_add(1, Ordering::SeqCst);
            if read == 1 {
                return Ok(Box::new(std::io::Cursor::new(self.replacement.clone())));
            }
        }
        self.inner.record(store, encoded_identity)
    }

    fn visit_records(
        &self,
        visitor: &mut dyn FnMut(StoreKind, &str) -> Result<(), VerificationFailure>,
    ) -> Result<(), VerificationFailure> {
        self.inner.visit_records(visitor)
    }
}

#[tokio::test]
async fn a_changed_inventory_body_is_not_a_fence() {
    let grant = br#"{"conversationId":"conv-1","principalId":"recv-1","credentialId":"cred-1"}"#;
    let log = DeletionLog::default()
        .append(
            DeletionFence::share_grant("conv-1", "recv-1", "cred-1", GrantRole::Comment, 1)
                .unwrap(),
        )
        .0;
    let bundle = publish(
        vec![Row {
            store: StoreKind::ShareGrants,
            identity: "grant-conv-1:recv-1".into(),
            bytes: Bytes::Public(grant.to_vec()),
        }],
        vec![],
        Some(log),
    )
    .await;
    let place = roots();
    let source = SecondInventory {
        inner: DirectoryVerificationSource::open(bundle.root.clone()).unwrap(),
        reads: AtomicUsize::new(0),
        replacement: DeletionLog::default().canonical().into_bytes(),
    };
    let error = stage_quarantine(
        &StageRequest {
            facts: person(RestoreAction::Stage),
            bundle: &source,
            format: &ExportFormat,
            verifier: &bundle.key,
            limits: VerificationLimits::default(),
            later_inventory: None,
            quarantine: &place.quarantine,
            live: &place.live,
        },
        &policy(),
        &MemoryAudit::open(),
        &DirectoryQuarantine,
        &DirectoryMaterializer,
        &KeyedInventory::new(bundle.key),
        &OsStagingIds,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Verification);
    assert_eq!(source.reads.load(Ordering::SeqCst), 2);
    assert!(!place.quarantine.exists());
    assert!(!place.live.exists());
}

struct StickyQuarantine;

impl QuarantineLayout for StickyQuarantine {
    fn begin_session(
        &self,
        root: &AnchoredRoot,
        relative: &str,
    ) -> Result<Box<dyn SessionAttempt + '_>, RestoreFault> {
        DirectoryQuarantine.begin_session(root, relative)
    }

    fn begin(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
    ) -> Result<Box<dyn Staging>, RestoreFault> {
        DirectoryQuarantine.begin(quarantine, live)
    }

    fn load(&self, root: &AnchoredRoot) -> Result<LoadedTree, RestoreFault> {
        DirectoryQuarantine.load(root)
    }

    fn promote(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
        loaded: &LoadedTree,
        generation: &str,
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded, generation)
    }

    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }

    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }

    fn exists(&self, path: &Path) -> Result<bool, RestoreFault> {
        DirectoryQuarantine.exists(path)
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.canonicalize(path)
    }

    fn anchor(&self, path: &Path) -> Result<AnchoredRoot, RestoreFault> {
        DirectoryQuarantine.anchor(path)
    }

    fn hold(&self, root: &AnchoredRoot) -> Result<Box<dyn RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }

    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }

    fn session_cut(&self, path: &Path, conversation_id: &str) -> Result<SessionCut, RestoreFault> {
        DirectoryQuarantine.session_cut(path, conversation_id)
    }
}

struct KeepsSession;

impl QuarantineLayout for KeepsSession {
    fn begin_session(
        &self,
        root: &AnchoredRoot,
        relative: &str,
    ) -> Result<Box<dyn SessionAttempt + '_>, RestoreFault> {
        TestAttempt::begin(TestRetirement::MoveAside, root, relative)
    }

    fn begin(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
    ) -> Result<Box<dyn Staging>, RestoreFault> {
        DirectoryQuarantine.begin(quarantine, live)
    }

    fn load(&self, root: &AnchoredRoot) -> Result<LoadedTree, RestoreFault> {
        DirectoryQuarantine.load(root)
    }

    fn promote(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
        loaded: &LoadedTree,
        generation: &str,
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded, generation)
    }

    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }

    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }

    fn exists(&self, path: &Path) -> Result<bool, RestoreFault> {
        DirectoryQuarantine.exists(path)
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.canonicalize(path)
    }

    fn anchor(&self, path: &Path) -> Result<AnchoredRoot, RestoreFault> {
        DirectoryQuarantine.anchor(path)
    }

    fn hold(&self, root: &AnchoredRoot) -> Result<Box<dyn RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }

    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }

    fn session_cut(&self, path: &Path, conversation_id: &str) -> Result<SessionCut, RestoreFault> {
        DirectoryQuarantine.session_cut(path, conversation_id)
    }
}

#[tokio::test]
async fn a_failed_cleanup_removes_the_journal() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    let audit = MemoryAudit::fail_after(1);
    let source = DirectoryVerificationSource::open(bundle.root.clone()).unwrap();
    let error = stage_quarantine(
        &StageRequest {
            facts: person(RestoreAction::Stage),
            bundle: &source,
            format: &ExportFormat,
            verifier: &bundle.key,
            limits: VerificationLimits::default(),
            later_inventory: None,
            quarantine: &place.quarantine,
            live: &place.live,
        },
        &policy(),
        &audit,
        &StickyQuarantine,
        &DirectoryMaterializer,
        &KeyedInventory::new(bundle.key),
        &OsStagingIds,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Retained);
    assert!(place.quarantine.is_dir());
    assert_eq!(
        fs::metadata(place.quarantine.join("journal.txt"))
            .unwrap()
            .len(),
        0
    );
    assert!(audit
        .lines()
        .iter()
        .all(|line| !(line.before == "verified" && line.after == "absent")));
    let accepted = accept(&place, &MemoryAudit::open(), &NoReceipts).unwrap_err();
    assert_eq!(accepted, RestoreFault::Tamper);
    assert!(!place.live.exists());
}

struct SpendMissed {
    keep_marker: bool,
}

impl QuarantineLayout for SpendMissed {
    fn begin_session(
        &self,
        root: &AnchoredRoot,
        relative: &str,
    ) -> Result<Box<dyn SessionAttempt + '_>, RestoreFault> {
        DirectoryQuarantine.begin_session(root, relative)
    }

    fn begin(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
    ) -> Result<Box<dyn Staging>, RestoreFault> {
        Ok(Box::new(FaultyStaging {
            inner: DirectoryQuarantine.begin(quarantine, live)?,
            fault: StageFault {
                ready: ReadyFault::After,
                retain: self.keep_marker,
            },
        }))
    }

    fn load(&self, root: &AnchoredRoot) -> Result<LoadedTree, RestoreFault> {
        DirectoryQuarantine.load(root)
    }

    fn promote(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
        loaded: &LoadedTree,
        generation: &str,
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded, generation)
    }

    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }

    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }

    fn exists(&self, path: &Path) -> Result<bool, RestoreFault> {
        DirectoryQuarantine.exists(path)
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.canonicalize(path)
    }

    fn anchor(&self, path: &Path) -> Result<AnchoredRoot, RestoreFault> {
        DirectoryQuarantine.anchor(path)
    }

    fn hold(&self, root: &AnchoredRoot) -> Result<Box<dyn RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }

    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }

    fn session_cut(&self, path: &Path, conversation_id: &str) -> Result<SessionCut, RestoreFault> {
        DirectoryQuarantine.session_cut(path, conversation_id)
    }
}

#[tokio::test]
async fn a_failed_spend_retires_the_readiness_marker() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    let audit = MemoryAudit::fail_after(2);
    let source = DirectoryVerificationSource::open(bundle.root.clone()).unwrap();
    let error = stage_quarantine(
        &StageRequest {
            facts: person(RestoreAction::Stage),
            bundle: &source,
            format: &ExportFormat,
            verifier: &bundle.key,
            limits: VerificationLimits::default(),
            later_inventory: None,
            quarantine: &place.quarantine,
            live: &place.live,
        },
        &policy(),
        &audit,
        &SpendMissed { keep_marker: false },
        &DirectoryMaterializer,
        &KeyedInventory::new(bundle.key),
        &OsStagingIds,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Retained);
    assert!(place.quarantine.join("journal.txt").is_file());
    assert_eq!(
        fs::metadata(place.quarantine.join("accept-ready"))
            .unwrap()
            .len(),
        0
    );
    assert!(audit.lines().iter().any(|line| line.cause == "sealed"));
    assert!(audit.lines().iter().all(|line| line.cause != "spent"));
    let accepted = accept(&place, &MemoryAudit::open(), &NoReceipts).unwrap_err();
    assert_eq!(accepted, RestoreFault::Tamper);
    assert!(!place.live.exists());
}

#[tokio::test]
async fn a_marker_that_survives_a_failed_spend_is_retained() {
    let session = sealed_session(b"session-v1");
    let bundle = publish(
        note_bundle_rows(),
        deferred_rows("conv-bulk", session.size, session.digest),
        None,
    )
    .await;
    let place = roots();
    let audit = MemoryAudit::fail_after(2);
    let source = DirectoryVerificationSource::open(bundle.root.clone()).unwrap();
    let error = stage_quarantine(
        &StageRequest {
            facts: person(RestoreAction::Stage),
            bundle: &source,
            format: &ExportFormat,
            verifier: &bundle.key,
            limits: VerificationLimits::default(),
            later_inventory: None,
            quarantine: &place.quarantine,
            live: &place.live,
        },
        &policy(),
        &audit,
        &SpendMissed { keep_marker: true },
        &DirectoryMaterializer,
        &KeyedInventory::new(bundle.key),
        &OsStagingIds,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Retained);
    let marker = fs::read(place.quarantine.join("accept-ready")).unwrap();
    assert!(marker.starts_with(b"stage-"));
    assert!(marker.ends_with(b"\n"));
    assert!(audit.lines().iter().all(|line| line.cause != "spent"));
    assert!(!place.live.exists());
    // Both physical retirement and the invalidating audit failed; the intent
    // alone must never make this still-present readiness marker authoritative.
    let resumed = audit.checkpoint();
    assert_eq!(
        accept(&place, &resumed, &NoReceipts).unwrap_err(),
        RestoreFault::Tamper
    );
    let deferred = FakeSource::matching(&session);
    assert_eq!(
        open_deferred_conversation(
            &OpenRequest {
                facts: person(RestoreAction::Fetch),
                root: &place.quarantine,
                conversation_id: "conv-bulk"
            },
            &policy(),
            &resumed,
            &DirectoryQuarantine,
            &deferred,
            &OsPublicationIds,
            &mut Quiet
        )
        .unwrap_err(),
        RestoreFault::Tamper
    );
    assert_eq!(*deferred.writes.lock().unwrap(), 0);
    let recovered = roots();
    stage(
        &bundle,
        &recovered.quarantine,
        &recovered.live,
        &resumed,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    accept(&recovered, &resumed, &NoReceipts).unwrap();
    assert!(
        !open_one(&recovered, "conv-bulk", &deferred, &resumed)
            .unwrap()
            .already_present
    );
    assert_eq!(*deferred.writes.lock().unwrap(), 1);
}

fn accept(
    place: &Roots,
    audit: &dyn RestoreAudit,
    receipts: &dyn LeaseReceipts,
) -> Result<(), RestoreFault> {
    accept_quarantine(
        &AcceptRequest {
            facts: person(RestoreAction::Accept),
            quarantine: &place.quarantine,
            live: &place.live,
            receipts,
        },
        &policy(),
        audit,
        &DirectoryQuarantine,
    )
    .map(|_| ())
}

fn tree_bytes(root: &Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                pending.push(entry.unwrap().path());
            }
        } else if path.is_file() {
            bytes.extend(fs::read(path).unwrap());
        }
    }
    bytes
}

fn holds(root: &Path, needle: &[u8]) -> bool {
    !needle.is_empty()
        && tree_bytes(root)
            .windows(needle.len())
            .any(|window| window == needle)
}

fn partial_names(parent: &Path) -> Vec<String> {
    fs::read_dir(parent)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".partial"))
        .collect()
}

fn note_bundle_rows() -> Vec<Row> {
    vec![Row {
        store: StoreKind::Notes,
        identity: "note-1".into(),
        bytes: Bytes::Public(b"a public note".to_vec()),
    }]
}

#[tokio::test]
async fn a_restore_refuses_to_use_live_data_as_quarantine() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    fs::create_dir(&place.live).unwrap();
    fs::write(place.live.join("sentinel"), b"live-bytes").unwrap();
    let audit = MemoryAudit::open();
    let nested = place.live.join("inside");
    let error = stage(
        &bundle,
        &nested,
        &place.live,
        &audit,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Paths);
    assert_eq!(
        fs::read(place.live.join("sentinel")).unwrap(),
        b"live-bytes"
    );
    assert!(!nested.exists());
    assert!(audit.lines().iter().any(|line| line.cause == "paths"));

    let place = roots();
    let error = stage(
        &bundle,
        &place.live,
        &place.live,
        &MemoryAudit::open(),
        person(RestoreAction::Stage),
        None,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Paths);
    assert!(!place.live.exists());

    let place = roots();
    let error = stage(
        &bundle,
        Path::new("quarantine"),
        &place.live,
        &MemoryAudit::open(),
        person(RestoreAction::Stage),
        None,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Paths);
}

#[tokio::test]
async fn a_tampered_bundle_does_not_touch_live_data() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let body = bundle
        .root
        .join("records")
        .join(StoreKind::Notes.as_str())
        .join("note-1");
    let mut bytes = fs::read(&body).unwrap();
    bytes[0] ^= 0xff;
    fs::write(&body, bytes).unwrap();
    let place = roots();
    fs::create_dir(&place.live).unwrap();
    fs::write(place.live.join("sentinel"), b"live-bytes").unwrap();
    let error = stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &MemoryAudit::open(),
        person(RestoreAction::Stage),
        None,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Verification);
    assert!(!place.quarantine.exists());
    assert_eq!(
        fs::read(place.live.join("sentinel")).unwrap(),
        b"live-bytes"
    );
    assert!(partial_names(place._keep.path()).is_empty());
}

#[test]
fn a_partial_copy_retires_owned_bytes_and_leaves_live_data() {
    let place = roots();
    fs::create_dir(&place.live).unwrap();
    fs::write(place.live.join("sentinel"), b"live-bytes").unwrap();
    {
        let quarantine = DirectoryQuarantine.anchor(&place.quarantine).unwrap();
        let live = DirectoryQuarantine.anchor(&place.live).unwrap();
        let mut staging = DirectoryQuarantine.begin(&quarantine, &live).unwrap();
        staging.write("records/notes/note-1", b"partial").unwrap();
    }
    assert!(!place.quarantine.exists());
    let partials = partial_names(place._keep.path());
    assert_eq!(partials.len(), 1);
    assert_eq!(
        fs::metadata(
            place
                ._keep
                .path()
                .join(&partials[0])
                .join("records/notes/note-1")
        )
        .unwrap()
        .len(),
        0
    );
    assert_eq!(
        fs::read(place.live.join("sentinel")).unwrap(),
        b"live-bytes"
    );
}

#[tokio::test]
async fn an_occupied_live_root_is_not_replaced() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    let audit = MemoryAudit::open();
    stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    fs::create_dir(&place.live).unwrap();
    fs::write(place.live.join("sentinel"), b"live-bytes").unwrap();
    let error = accept(&place, &audit, &NoReceipts).unwrap_err();
    assert_eq!(error, RestoreFault::LiveOccupied);
    assert_eq!(
        fs::read(place.live.join("sentinel")).unwrap(),
        b"live-bytes"
    );
    assert!(place.quarantine.join("journal.txt").is_file());
}

#[test]
fn an_empty_live_directory_is_not_replaced() {
    let place = roots();
    fs::create_dir_all(place.quarantine.join("records")).unwrap();
    fs::write(place.quarantine.join("journal.txt"), b"sealed\n").unwrap();
    fs::create_dir(&place.live).unwrap();
    let quarantine = DirectoryQuarantine.anchor(&place.quarantine).unwrap();
    let live = DirectoryQuarantine.anchor(&place.live).unwrap();
    fs::write(place.quarantine.join("accept-ready"), b"stage-test\n").unwrap();
    let loaded = DirectoryQuarantine.load(&quarantine).unwrap();
    let error = DirectoryQuarantine
        .promote(&quarantine, &live, &loaded, "stage-test")
        .unwrap_err();
    assert_eq!(error, RestoreFault::Retained);
    let copies = fs::read_dir(place._keep.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(".sealed-")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        copies.len(),
        1,
        "the refused copy retains its owned namespace"
    );
    let retired = snapshot_tree_files(&copies[0]);
    assert!(!retired.is_empty());
    assert!(retired.iter().all(|(_, bytes)| bytes.is_empty()));
    assert_eq!(
        fs::read(place.quarantine.join("journal.txt")).unwrap(),
        b"sealed\n"
    );
    assert!(place.quarantine.join("journal.txt").is_file());
    assert!(fs::read_dir(&place.live).unwrap().next().is_none());
}

#[test]
fn a_swapped_quarantine_directory_is_not_promoted() {
    let place = roots();
    fs::create_dir_all(&place.quarantine).unwrap();
    fs::write(place.quarantine.join("journal.txt"), b"sealed\n").unwrap();
    let quarantine = DirectoryQuarantine.anchor(&place.quarantine).unwrap();
    let live = DirectoryQuarantine.anchor(&place.live).unwrap();
    fs::write(place.quarantine.join("accept-ready"), b"stage-test\n").unwrap();
    let loaded = DirectoryQuarantine.load(&quarantine).unwrap();
    let saved = place.quarantine.with_extension("saved");
    fs::rename(&place.quarantine, &saved).unwrap();
    fs::create_dir(&place.quarantine).unwrap();
    fs::write(place.quarantine.join("journal.txt"), b"other\n").unwrap();
    let error = DirectoryQuarantine
        .promote(&quarantine, &live, &loaded, "stage-test")
        .unwrap_err();
    assert_eq!(error, RestoreFault::Tamper);
    assert!(!place.live.exists());
    assert_eq!(
        fs::read(place.quarantine.join("journal.txt")).unwrap(),
        b"other\n"
    );
    assert_eq!(fs::read(saved.join("journal.txt")).unwrap(), b"sealed\n");
}

#[test]
fn an_open_descriptor_does_not_change_the_promoted_file() {
    let place = roots();
    fs::create_dir_all(place.quarantine.join("records")).unwrap();
    fs::write(place.quarantine.join("journal.txt"), b"sealed\n").unwrap();
    fs::write(place.quarantine.join("records/note"), b"one").unwrap();
    let quarantine = DirectoryQuarantine.anchor(&place.quarantine).unwrap();
    let live = DirectoryQuarantine.anchor(&place.live).unwrap();
    fs::write(place.quarantine.join("accept-ready"), b"stage-test\n").unwrap();
    let loaded = DirectoryQuarantine.load(&quarantine).unwrap();
    let mut held = fs::OpenOptions::new()
        .write(true)
        .open(place.quarantine.join("records/note"))
        .unwrap();
    DirectoryQuarantine
        .promote(&quarantine, &live, &loaded, "stage-test")
        .unwrap();
    use std::io::{Seek, SeekFrom, Write};
    held.seek(SeekFrom::Start(0)).unwrap();
    held.write_all(b"two").unwrap();
    held.sync_all().unwrap();
    assert_eq!(fs::read(place.live.join("records/note")).unwrap(), b"one");
    assert_eq!(
        fs::read(place.live.join("accept-ready")).unwrap(),
        b"stage-test\n"
    );
}

#[test]
fn a_changed_quarantine_file_is_not_promoted() {
    let place = roots();
    fs::create_dir_all(place.quarantine.join("records")).unwrap();
    fs::write(place.quarantine.join("journal.txt"), b"sealed\n").unwrap();
    fs::write(place.quarantine.join("records/note"), b"one").unwrap();
    let quarantine = DirectoryQuarantine.anchor(&place.quarantine).unwrap();
    let live = DirectoryQuarantine.anchor(&place.live).unwrap();
    fs::write(place.quarantine.join("accept-ready"), b"stage-test\n").unwrap();
    let loaded = DirectoryQuarantine.load(&quarantine).unwrap();
    fs::write(place.quarantine.join("records/note"), b"two").unwrap();
    let error = DirectoryQuarantine
        .promote(&quarantine, &live, &loaded, "stage-test")
        .unwrap_err();
    assert_eq!(error, RestoreFault::Tamper);
    assert!(!place.live.exists());
    assert_eq!(
        fs::read(place.quarantine.join("records/note")).unwrap(),
        b"two"
    );
}

#[test]
fn a_folding_volume_locks_spellings_in_one_order() {
    let (first, second, third, fourth) = folding_lock_spellings();
    let (earlier, later) =
        ordered_roots(&first, VolumeCase::Unicode, &second, VolumeCase::Unicode).unwrap();
    let (again, again_later) =
        ordered_roots(&third, VolumeCase::Unicode, &fourth, VolumeCase::Unicode).unwrap();
    assert_eq!(earlier, first);
    assert_eq!(later, second);
    assert_eq!(again, third);
    assert_eq!(again_later, fourth);
}

fn folding_lock_spellings() -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    #[cfg(windows)]
    {
        (
            PathBuf::from(r"C:\restore\a"),
            PathBuf::from(r"C:\restore\B"),
            PathBuf::from(r"C:\restore\A"),
            PathBuf::from(r"C:\restore\b"),
        )
    }
    #[cfg(not(windows))]
    {
        (
            PathBuf::from("/restore/a"),
            PathBuf::from("/restore/B"),
            PathBuf::from("/restore/A"),
            PathBuf::from("/restore/b"),
        )
    }
}

#[test]
fn a_discarded_partial_directory_reports_its_retained_namespace() {
    let place = roots();
    let quarantine = DirectoryQuarantine.anchor(&place.quarantine).unwrap();
    let live = DirectoryQuarantine.anchor(&place.live).unwrap();
    let mut staging = DirectoryQuarantine.begin(&quarantine, &live).unwrap();
    staging.write("records/notes/note-1", b"partial").unwrap();
    assert_eq!(staging.discard().unwrap_err(), RestoreFault::Retained);
    let partials = partial_names(place._keep.path());
    assert_eq!(partials.len(), 1);
    assert_eq!(
        fs::metadata(
            place
                ._keep
                .path()
                .join(&partials[0])
                .join("records/notes/note-1")
        )
        .unwrap()
        .len(),
        0
    );
    assert!(!place.quarantine.exists());
}

#[tokio::test]
async fn a_tampered_quarantine_is_not_promoted() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    let audit = MemoryAudit::open();
    stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    let sealed = place.quarantine.join("deferred.txt");
    let mut bytes = fs::read(&sealed).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    fs::write(&sealed, bytes).unwrap();
    let error = accept(&place, &audit, &NoReceipts).unwrap_err();
    assert_eq!(error, RestoreFault::Tamper);
    assert!(!place.live.exists());
    assert!(place.quarantine.join("journal.txt").is_file());
}

#[tokio::test]
async fn a_lock_file_is_not_the_live_path() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let keep = tempfile::tempdir().unwrap();
    let quarantine = keep.path().join("q");
    let live = keep.path().join("q.restore-lock");
    let error = stage(
        &bundle,
        &quarantine,
        &live,
        &MemoryAudit::open(),
        person(RestoreAction::Stage),
        None,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Paths);
    assert!(!live.exists());
}

#[tokio::test]
async fn an_injected_publication_suffix_is_not_promoted() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    let audit = MemoryAudit::open();
    stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    let injected = place
        .quarantine
        .join("records/share-grants/revoked.publication");
    fs::create_dir_all(injected.parent().unwrap()).unwrap();
    fs::write(&injected, b"not-a-session\n").unwrap();
    let error = accept(&place, &audit, &NoReceipts).unwrap_err();
    assert_eq!(error, RestoreFault::Tamper);
    assert!(!place.live.exists());
    assert!(injected.is_file());
}

#[tokio::test]
async fn a_resealed_quarantine_is_not_promoted() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    let audit = MemoryAudit::open();
    stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    let journal_path = place.quarantine.join("journal.txt");
    let original = fs::read_to_string(&journal_path).unwrap();
    let parsed = read_journal(&original).unwrap();
    fs::write(place.quarantine.join("identity.txt"), b"tampered\n").unwrap();
    let mut replaced = false;
    let files = parsed
        .files
        .into_iter()
        .map(|mut file| {
            if file.path == "identity.txt" {
                file.fingerprint = Fingerprint::of(b"tampered\n");
                replaced = true;
            }
            file
        })
        .collect::<Vec<_>>();
    assert!(replaced);
    let rewritten = encode_journal(parsed.freshness, &parsed.leases, &files).unwrap();
    fs::write(&journal_path, &rewritten).unwrap();
    let sealed = audit
        .lines()
        .into_iter()
        .find(|line| line.cause == "sealed")
        .unwrap();
    assert_ne!(
        sealed.attestations,
        Fingerprint::of(rewritten.as_bytes()).hex()
    );
    let error = accept(&place, &audit, &NoReceipts).unwrap_err();
    assert_eq!(error, RestoreFault::Tamper);
    assert!(!place.live.exists());
    assert!(journal_path.is_file());
}

struct FailingStage;

impl StagingIds for FailingStage {
    fn staging_id(&self) -> Result<String, RestoreFault> {
        Err(RestoreFault::Unreadable)
    }
}

#[tokio::test]
async fn a_failed_staging_id_keeps_an_unremoved_tree() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let kept = roots();
    let source = DirectoryVerificationSource::open(bundle.root.clone()).unwrap();
    let kept_audit = MemoryAudit::open();
    let error = stage_quarantine(
        &StageRequest {
            facts: person(RestoreAction::Stage),
            bundle: &source,
            format: &ExportFormat,
            verifier: &bundle.key,
            limits: VerificationLimits::default(),
            later_inventory: None,
            quarantine: &kept.quarantine,
            live: &kept.live,
        },
        &policy(),
        &kept_audit,
        &ReadyFails { keep: true },
        &DirectoryMaterializer,
        &KeyedInventory::new(bundle.key),
        &FailingStage,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Retained);
    assert!(kept.quarantine.join("journal.txt").is_file());
    assert!(kept_audit.lines().iter().any(|line| {
        line.before == "absent" && line.after == "unready" && line.cause == "unreadable"
    }));
    assert!(kept_audit.lines().iter().all(|line| {
        !(line.decision == "effect" && line.before == "unchanged" && line.after == "unchanged")
    }));

    let removed = roots();
    let removed_audit = MemoryAudit::open();
    let error = stage_quarantine(
        &StageRequest {
            facts: person(RestoreAction::Stage),
            bundle: &source,
            format: &ExportFormat,
            verifier: &bundle.key,
            limits: VerificationLimits::default(),
            later_inventory: None,
            quarantine: &removed.quarantine,
            live: &removed.live,
        },
        &policy(),
        &removed_audit,
        &ReadyFails { keep: false },
        &DirectoryMaterializer,
        &KeyedInventory::new(bundle.key),
        &FailingStage,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Retained);
    assert_eq!(
        fs::metadata(removed.quarantine.join("journal.txt"))
            .unwrap()
            .len(),
        0
    );
    assert!(removed_audit.lines().iter().any(|line| {
        line.decision == "effect"
            && line.before == "absent"
            && line.after == "unready"
            && line.cause == "unreadable"
    }));
}

struct FixedStage(&'static str);

impl StagingIds for FixedStage {
    fn staging_id(&self) -> Result<String, RestoreFault> {
        Ok(self.0.to_owned())
    }
}

fn stage_with_id(
    bundle: &Bundle,
    quarantine: &Path,
    live: &Path,
    audit: &dyn RestoreAudit,
    ids: &dyn StagingIds,
) -> Result<StageReport, RestoreFault> {
    let source = DirectoryVerificationSource::open(bundle.root.clone()).unwrap();
    stage_quarantine(
        &StageRequest {
            facts: person(RestoreAction::Stage),
            bundle: &source,
            format: &ExportFormat,
            verifier: &bundle.key,
            limits: VerificationLimits::default(),
            later_inventory: None,
            quarantine,
            live,
        },
        &policy(),
        audit,
        &DirectoryQuarantine,
        &DirectoryMaterializer,
        &KeyedInventory::new(bundle.key),
        ids,
    )
}

#[tokio::test]
async fn a_spent_staging_id_is_not_promoted() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    let audit = MemoryAudit::open();
    stage_with_id(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        &FixedStage("stage-fixed"),
    )
    .unwrap();
    let journal = fs::read(place.quarantine.join("journal.txt")).unwrap();
    let digest = Fingerprint::of(&journal).hex();
    let sealed = audit
        .lines()
        .into_iter()
        .find(|line| line.cause == "sealed")
        .unwrap();
    assert_eq!(
        seal_staging_id(&sealed.attestations, &digest).as_deref(),
        Some("stage-fixed")
    );
    assert_eq!(
        fs::read(place.quarantine.join("accept-ready")).unwrap(),
        b"stage-fixed\n"
    );
    accept(&place, &audit, &NoReceipts).unwrap();
    assert_eq!(
        fs::metadata(place.quarantine.join("accept-ready"))
            .unwrap()
            .len(),
        0
    );
    assert!(audit
        .lines()
        .iter()
        .any(|line| line.after == "spent" && line.attestations == "stage-fixed"));
    let source = DirectoryQuarantine.anchor(&place.quarantine).unwrap();
    assert_eq!(
        audit
            .sealed(&source.display().to_string(), &digest)
            .unwrap(),
        None
    );
    fs::rename(&place.quarantine, place._keep.path().join("spent-source")).unwrap();
    fs::rename(&place.live, &place.quarantine).unwrap();
    assert_eq!(
        fs::read(place.quarantine.join("accept-ready")).unwrap(),
        b"stage-fixed\n"
    );
    let error = accept(&place, &audit, &NoReceipts).unwrap_err();
    assert_eq!(error, RestoreFault::Tamper);
    assert!(!place.live.exists());
    assert!(place.quarantine.join("journal.txt").is_file());
}

#[tokio::test]
async fn a_constant_ready_marker_is_not_a_generation() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    let audit = MemoryAudit::open();
    stage_with_id(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        &FixedStage("stage-fixed"),
    )
    .unwrap();
    // Deliberately mutate trusted permission metadata to prepare this malformed
    // fixture; ordinary namespace writers cannot change the protected marker.
    let marker = place.quarantine.join("accept-ready");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let denied = fs::write(&marker, b"ready\n").unwrap_err();
        assert_eq!(denied.raw_os_error(), Some(5));
        assert_eq!(fs::read(&marker).unwrap(), b"stage-fixed\n");
        const WRITE_DAC: u32 = 0x0004_0000;
        let metadata_authority = fs::OpenOptions::new()
            .access_mode(WRITE_DAC)
            .open(&marker)
            .unwrap();
        nessa_local_storage::release_file_writes(&metadata_authority).unwrap();
    }
    fs::write(&marker, b"ready\n").unwrap();
    let error = accept(&place, &audit, &NoReceipts).unwrap_err();
    assert_eq!(error, RestoreFault::Tamper);
    assert!(!place.live.exists());
}

#[tokio::test]
async fn quarantine_records_posture_and_repair_markers() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    let audit = MemoryAudit::open();
    stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    assert_eq!(
        root_posture(&place.quarantine, &place.live, &DirectoryQuarantine).unwrap(),
        Posture::Quarantined
    );
    accept(&place, &audit, &NoReceipts).unwrap();
    assert_eq!(
        fs::read(place.live.join("replay.blocked")).unwrap(),
        b"blocked\n"
    );
    assert_eq!(
        fs::read(place.live.join("identity.txt")).unwrap(),
        b"re-pair\n"
    );
    assert_eq!(
        root_posture(&place.live, &place.live, &DirectoryQuarantine).unwrap(),
        Posture::Live
    );
    let before = fs::read(place.live.join("records/notes/note-1")).unwrap();
    let error = accept(&place, &audit, &NoReceipts).unwrap_err();
    assert_eq!(error, RestoreFault::Tamper);
    assert_eq!(
        fs::read(place.live.join("records/notes/note-1")).unwrap(),
        before
    );
    assert_eq!(
        fs::read(place.live.join("replay.blocked")).unwrap(),
        b"blocked\n"
    );
}

#[tokio::test]
async fn a_device_may_not_stage_a_restore() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    let audit = MemoryAudit::open();
    let cedar = policy();
    for kind in ["device", "peer", "environment-peer", "worker"] {
        let decision = cedar.evaluate(&RestoreFacts {
            kind,
            cause: InventoryCause::Operator,
            action: RestoreAction::Stage,
        });
        assert!(
            matches!(decision, InventoryDecision::Deny(_)),
            "{kind} was permitted"
        );
    }
    let allowed = cedar.evaluate(&RestoreFacts {
        kind: "surface",
        cause: InventoryCause::Operator,
        action: RestoreAction::Accept,
    });
    assert!(matches!(allowed, InventoryDecision::Allow { .. }));
    let error = stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        RestoreFacts {
            kind: "device",
            cause: InventoryCause::Operator,
            action: RestoreAction::Stage,
        },
        None,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Denied);
    assert!(!place.quarantine.exists());
    assert!(!place.live.exists());
    let lines = audit.lines();
    assert_eq!(lines[0].decision, "deny");
    assert_eq!(lines[0].before, "absent");
    assert_eq!(lines[0].after, "absent");
    assert!(lines[0]
        .policies
        .iter()
        .any(|id| id == "restore.refuse.device"));
}

#[tokio::test]
async fn a_caller_supplied_action_does_not_choose_the_policy() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    let audit = MemoryAudit::open();
    stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        RestoreFacts {
            kind: "person",
            cause: InventoryCause::Operator,
            action: RestoreAction::Fetch,
        },
        None,
    )
    .unwrap();
    let policies = &audit.lines()[0].policies;
    assert!(policies.iter().any(|id| id == "restore.stage.person"));
    assert!(policies.iter().all(|id| !id.contains("fetch")));
}

#[tokio::test]
async fn a_failed_decision_audit_does_not_stage() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    let error = stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &MemoryAudit::fail_after(0),
        person(RestoreAction::Stage),
        None,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Audit);
    assert!(!place.quarantine.exists());
    assert!(partial_names(place._keep.path()).is_empty());
}

#[tokio::test]
async fn a_failed_outcome_audit_retires_the_owned_quarantine_bytes() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    let error = stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &MemoryAudit::fail_after(1),
        person(RestoreAction::Stage),
        None,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Retained);
    let files = snapshot_tree_files(&place.quarantine);
    assert!(!files.is_empty());
    assert!(files.iter().all(|(_, bytes)| bytes.is_empty()));
    assert!(!place.quarantine.join("accept-ready").exists());
    assert!(!place.live.exists());
    assert!(partial_names(place._keep.path()).is_empty());
}

#[tokio::test]
async fn an_older_inventory_is_refused() {
    let fence = DeletionFence::conversation("conv-1").unwrap();
    let cut = {
        let mut log = DeletionLog::default();
        log = log.append(fence).0;
        log
    };
    let bundle = publish(note_bundle_rows(), vec![], Some(cut)).await;
    let later = seal_inventory(bundle.key, &DeletionLog::default());
    let place = roots();
    let error = stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &MemoryAudit::open(),
        person(RestoreAction::Stage),
        Some(&later),
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Rollback);
    assert!(!place.quarantine.exists());
    assert!(!place.live.exists());
}

#[tokio::test]
async fn a_revoked_grant_is_absent_from_quarantine() {
    let grant = br#"{"conversationId":"conv-1","receiverId":"recv-1","credentialId":"cred-1"}"#;
    let credential = br#"{"id":"cred-9","kind":"device","fingerprint":"ab"}"#;
    let conversation = br#"{"id":"conv-1","title":"kept-out"}"#;
    let summary = br#"{"conversationId":"conv-1","text":"kept-out"}"#;
    let tombstone = br#"{"conversationId":"conv-1","tombstone":"stays"}"#;
    let mut log = DeletionLog::default();
    log = log
        .append(
            DeletionFence::share_grant("conv-1", "recv-1", "cred-1", GrantRole::Read, 4).unwrap(),
        )
        .0;
    log = log
        .append(
            DeletionFence::credential(CredentialKind::Device, "cred-9", Fingerprint::of(b"public"))
                .unwrap(),
        )
        .0;
    log = log.append(DeletionFence::conversation("conv-1").unwrap()).0;
    let bundle = publish(
        vec![
            Row {
                store: StoreKind::ShareGrants,
                identity: "grant-conv-1:recv-1".into(),
                bytes: Bytes::Public(grant.to_vec()),
            },
            Row {
                store: StoreKind::DeviceCredentials,
                identity: "cred-9".into(),
                bytes: Bytes::Public(credential.to_vec()),
            },
            Row {
                store: StoreKind::ConversationMetadata,
                identity: "conversation-conv-1".into(),
                bytes: Bytes::Public(conversation.to_vec()),
            },
            Row {
                store: StoreKind::ConversationMetadata,
                identity: "summary-conv-1".into(),
                bytes: Bytes::Public(summary.to_vec()),
            },
            Row {
                store: StoreKind::ConversationMetadata,
                identity: "tombstone-conv-1".into(),
                bytes: Bytes::Public(tombstone.to_vec()),
            },
            Row {
                store: StoreKind::Notes,
                identity: "secret-note".into(),
                bytes: Bytes::Secret(b"DO-NOT-RESTORE-SECRET".to_vec()),
            },
        ],
        vec![],
        Some(log),
    )
    .await;
    let place = roots();
    stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &MemoryAudit::open(),
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    let tree = tree_bytes(&place.quarantine);
    assert!(!holds(&place.quarantine, grant));
    assert!(!holds(&place.quarantine, credential));
    assert!(!holds(&place.quarantine, conversation));
    assert!(!holds(&place.quarantine, summary));
    assert!(tree
        .windows(tombstone.len())
        .any(|window| window == tombstone));
    assert!(!holds(&place.quarantine, b"DO-NOT-RESTORE-SECRET"));
    assert!(!place.live.exists());
}

#[tokio::test]
async fn a_worker_fence_omits_the_environment_peer_credential() {
    let credential = br#"{"id":"cred-w","kind":"environment-peer","fingerprint":"ab"}"#;
    let log = DeletionLog::default()
        .append(
            DeletionFence::credential(CredentialKind::Worker, "cred-w", Fingerprint::of(b"public"))
                .unwrap(),
        )
        .0;
    let bundle = publish(
        vec![Row {
            store: StoreKind::EnvironmentPeerCredentials,
            identity: "cred-w".into(),
            bytes: Bytes::Public(credential.to_vec()),
        }],
        vec![],
        Some(log),
    )
    .await;
    let place = roots();
    stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &MemoryAudit::open(),
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    assert!(!holds(&place.quarantine, credential));
    assert!(!place.live.exists());
}

#[tokio::test]
async fn a_reissued_grant_is_kept_and_the_revoked_epoch_is_not() {
    let revoked = br#"{"conversationId":"conv-1","receiverId":"recv-1","credentialId":"cred-1","policyRevision":4}"#;
    let renewed = br#"{"conversationId":"conv-1","receiverId":"recv-1","credentialId":"cred-1","policyRevision":9}"#;
    let log = DeletionLog::default()
        .append(
            DeletionFence::share_grant("conv-1", "recv-1", "cred-1", GrantRole::Read, 4).unwrap(),
        )
        .0;
    let bundle = publish(
        vec![
            Row {
                store: StoreKind::ShareGrants,
                identity: "grant-revoked".into(),
                bytes: Bytes::Public(revoked.to_vec()),
            },
            Row {
                store: StoreKind::ShareGrants,
                identity: "grant-renewed".into(),
                bytes: Bytes::Public(renewed.to_vec()),
            },
        ],
        vec![],
        Some(log),
    )
    .await;
    let place = roots();
    stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &MemoryAudit::open(),
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    assert!(!holds(&place.quarantine, revoked));
    assert!(holds(&place.quarantine, renewed));
}

struct OtherQuarantine {
    quarantine: PathBuf,
}

impl Staging for OtherQuarantine {
    fn write(&mut self, _relative: &str, _bytes: &[u8]) -> Result<(), RestoreFault> {
        Ok(())
    }

    fn finish(self: Box<Self>, _journal: &str) -> Result<Box<dyn StagedTreeOwner>, RestoreFault> {
        fs::create_dir_all(&self.quarantine).map_err(|_| RestoreFault::Unreadable)?;
        fs::write(self.quarantine.join("owned-by-other"), b"keep")
            .map_err(|_| RestoreFault::Unreadable)?;
        Err(RestoreFault::QuarantineOccupied)
    }

    fn discard(self: Box<Self>) -> Result<(), RestoreFault> {
        Ok(())
    }
}

struct ReportsOccupied {
    quarantine: PathBuf,
}

impl QuarantineLayout for ReportsOccupied {
    fn begin_session(
        &self,
        root: &AnchoredRoot,
        relative: &str,
    ) -> Result<Box<dyn SessionAttempt + '_>, RestoreFault> {
        DirectoryQuarantine.begin_session(root, relative)
    }

    fn begin(
        &self,
        _quarantine: &AnchoredRoot,
        _live: &AnchoredRoot,
    ) -> Result<Box<dyn Staging>, RestoreFault> {
        Ok(Box::new(OtherQuarantine {
            quarantine: self.quarantine.clone(),
        }))
    }
    fn load(&self, root: &AnchoredRoot) -> Result<LoadedTree, RestoreFault> {
        DirectoryQuarantine.load(root)
    }
    fn promote(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
        loaded: &LoadedTree,
        generation: &str,
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded, generation)
    }
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }

    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }

    fn exists(&self, path: &Path) -> Result<bool, RestoreFault> {
        DirectoryQuarantine.exists(path)
    }
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.canonicalize(path)
    }
    fn anchor(&self, path: &Path) -> Result<AnchoredRoot, RestoreFault> {
        DirectoryQuarantine.anchor(path)
    }
    fn hold(&self, root: &AnchoredRoot) -> Result<Box<dyn RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }
    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }

    fn session_cut(&self, path: &Path, conversation_id: &str) -> Result<SessionCut, RestoreFault> {
        DirectoryQuarantine.session_cut(path, conversation_id)
    }
}

#[tokio::test]
async fn a_quarantine_owned_by_someone_else_is_not_deleted() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    let source = DirectoryVerificationSource::open(bundle.root.clone()).unwrap();
    let error = stage_quarantine(
        &StageRequest {
            facts: person(RestoreAction::Stage),
            bundle: &source,
            format: &ExportFormat,
            verifier: &bundle.key,
            limits: VerificationLimits::default(),
            later_inventory: None,
            quarantine: &place.quarantine,
            live: &place.live,
        },
        &policy(),
        &MemoryAudit::open(),
        &ReportsOccupied {
            quarantine: place.quarantine.clone(),
        },
        &DirectoryMaterializer,
        &KeyedInventory::new(bundle.key),
        &OsStagingIds,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::QuarantineOccupied);
    assert_eq!(
        fs::read(place.quarantine.join("owned-by-other")).unwrap(),
        b"keep"
    );
    assert!(!place.live.exists());
}

struct ReadyFails {
    keep: bool,
}

impl QuarantineLayout for ReadyFails {
    fn begin_session(
        &self,
        root: &AnchoredRoot,
        relative: &str,
    ) -> Result<Box<dyn SessionAttempt + '_>, RestoreFault> {
        DirectoryQuarantine.begin_session(root, relative)
    }

    fn begin(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
    ) -> Result<Box<dyn Staging>, RestoreFault> {
        Ok(Box::new(FaultyStaging {
            inner: DirectoryQuarantine.begin(quarantine, live)?,
            fault: StageFault {
                ready: ReadyFault::Before,
                retain: self.keep,
            },
        }))
    }
    fn load(&self, root: &AnchoredRoot) -> Result<LoadedTree, RestoreFault> {
        DirectoryQuarantine.load(root)
    }
    fn promote(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
        loaded: &LoadedTree,
        generation: &str,
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded, generation)
    }
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }

    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }

    fn exists(&self, path: &Path) -> Result<bool, RestoreFault> {
        DirectoryQuarantine.exists(path)
    }
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.canonicalize(path)
    }
    fn anchor(&self, path: &Path) -> Result<AnchoredRoot, RestoreFault> {
        DirectoryQuarantine.anchor(path)
    }
    fn hold(&self, root: &AnchoredRoot) -> Result<Box<dyn RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }
    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }

    fn session_cut(&self, path: &Path, conversation_id: &str) -> Result<SessionCut, RestoreFault> {
        DirectoryQuarantine.session_cut(path, conversation_id)
    }
}

#[tokio::test]
async fn a_failed_readiness_mark_records_whether_the_tree_remains() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let removed = roots();
    let source = DirectoryVerificationSource::open(bundle.root.clone()).unwrap();
    let removed_audit = MemoryAudit::open();
    let error = stage_quarantine(
        &StageRequest {
            facts: person(RestoreAction::Stage),
            bundle: &source,
            format: &ExportFormat,
            verifier: &bundle.key,
            limits: VerificationLimits::default(),
            later_inventory: None,
            quarantine: &removed.quarantine,
            live: &removed.live,
        },
        &policy(),
        &removed_audit,
        &ReadyFails { keep: false },
        &DirectoryMaterializer,
        &KeyedInventory::new(bundle.key),
        &OsStagingIds,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Retained);
    assert_eq!(
        fs::metadata(removed.quarantine.join("journal.txt"))
            .unwrap()
            .len(),
        0
    );
    assert!(removed_audit.lines().iter().any(|line| {
        line.before == "verified" && line.after == "unready" && line.cause == "unreadable"
    }));

    let kept = roots();
    let kept_audit = MemoryAudit::open();
    let error = stage_quarantine(
        &StageRequest {
            facts: person(RestoreAction::Stage),
            bundle: &source,
            format: &ExportFormat,
            verifier: &bundle.key,
            limits: VerificationLimits::default(),
            later_inventory: None,
            quarantine: &kept.quarantine,
            live: &kept.live,
        },
        &policy(),
        &kept_audit,
        &ReadyFails { keep: true },
        &DirectoryMaterializer,
        &KeyedInventory::new(bundle.key),
        &OsStagingIds,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Retained);
    assert!(kept.quarantine.join("journal.txt").is_file());
    let digest = Fingerprint::of(&fs::read(kept.quarantine.join("journal.txt")).unwrap()).hex();
    let staging_id = kept_audit
        .lines()
        .iter()
        .find(|line| line.cause == "sealed")
        .and_then(|line| seal_staging_id(&line.attestations, &digest))
        .unwrap();
    assert!(kept_audit.lines().iter().any(|line| {
        line.before == "verified"
            && line.after == "unready"
            && line.cause == "unreadable"
            && line.attestations == staging_id
    }));
    fs::write(
        kept.quarantine.join("accept-ready"),
        format!("{staging_id}\n"),
    )
    .unwrap();
    let accepted = accept(&kept, &kept_audit, &NoReceipts).unwrap_err();
    assert_eq!(accepted, RestoreFault::Tamper);
    assert!(!kept.live.exists());
}

#[tokio::test]
async fn a_newer_fence_omits_the_grant() {
    let grant = br#"{"conversationId":"conv-2","principalId":"recv-2","credentialId":"cred-2"}"#;
    let bundle = publish(
        vec![Row {
            store: StoreKind::ShareGrants,
            identity: "grant-conv-2:recv-2".into(),
            bytes: Bytes::Public(grant.to_vec()),
        }],
        vec![],
        None,
    )
    .await;
    let later_log = DeletionLog::default()
        .append(
            DeletionFence::share_grant("conv-2", "recv-2", "cred-2", GrantRole::Comment, 9)
                .unwrap(),
        )
        .0;
    let later = seal_inventory(bundle.key, &later_log);
    let place = roots();
    let report = stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &MemoryAudit::open(),
        person(RestoreAction::Stage),
        Some(&later),
    )
    .unwrap();
    assert_eq!(report.freshness, Freshness::Superset);
    assert!(!holds(&place.quarantine, grant));
    assert!(place.quarantine.join("journal.txt").is_file());
}

#[tokio::test]
async fn an_open_lease_is_interrupted_in_quarantine() {
    let lease = br#"{"kind":"granted","lease":"lease-1"}"#;
    let bundle = publish(
        vec![Row {
            store: StoreKind::LeaseRecords,
            identity: "line-1".into(),
            bytes: Bytes::Public(lease.to_vec()),
        }],
        vec![],
        None,
    )
    .await;
    let place = roots();
    let audit = MemoryAudit::open();
    let report = stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    assert_eq!(report.leases, vec!["lease-1".to_owned()]);
    let restored = fs::read(place.quarantine.join("records/lease-records/line-1")).unwrap();
    let text = String::from_utf8(restored).unwrap();
    assert!(text.contains("\"kind\":\"granted\""));
    assert!(text.contains("\"kind\":\"ended\""));
    assert!(text.contains("\"outcome\":\"uncertain\""));
    assert!(text.contains("\"cause\":\"lost\""));
    let error = accept(&place, &audit, &NoReceipts).unwrap_err();
    assert_eq!(error, RestoreFault::ExecutorsUnfenced);
    assert!(!place.live.exists());
    accept(
        &place,
        &audit,
        &MapReceipts(&[("lease-1", "stopped-host-1")]),
    )
    .unwrap();
    assert!(place.live.join("records/lease-records/line-1").is_file());
    assert_eq!(
        fs::metadata(place.quarantine.join("accept-ready"))
            .unwrap()
            .len(),
        0
    );
    assert!(audit.lines().iter().any(|line| line.after == "spent"));
}

#[tokio::test]
async fn a_closed_lease_across_lines_needs_no_receipt() {
    let granted = br#"{"kind":"granted","lease":"lease-closed"}"#;
    let ended = br#"{"kind":"ended","lease":"lease-closed","cleanup":{"outcome":"uncertain"},"cause":"lost"}"#;
    let bundle = publish(
        vec![
            Row {
                store: StoreKind::LeaseRecords,
                identity: "line-1".into(),
                bytes: Bytes::Public(granted.to_vec()),
            },
            Row {
                store: StoreKind::LeaseRecords,
                identity: "line-2".into(),
                bytes: Bytes::Public(ended.to_vec()),
            },
        ],
        vec![],
        None,
    )
    .await;
    let place = roots();
    let audit = MemoryAudit::open();
    let report = stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    assert!(report.leases.is_empty());
    assert_eq!(
        fs::read(place.quarantine.join("records/lease-records/line-1")).unwrap(),
        granted
    );
    accept(&place, &audit, &NoReceipts).unwrap();
    assert!(place.live.join("records/lease-records/line-2").is_file());
}

#[tokio::test]
async fn a_reopened_lease_is_closed_on_its_last_line() {
    let granted = br#"{"kind":"granted","lease":"lease-again"}"#;
    let ended = br#"{"kind":"ended","lease":"lease-again","cleanup":{"outcome":"uncertain"},"cause":"lost"}"#;
    let bundle = publish(
        vec![
            Row {
                store: StoreKind::LeaseRecords,
                identity: "line-1".into(),
                bytes: Bytes::Public(granted.to_vec()),
            },
            Row {
                store: StoreKind::LeaseRecords,
                identity: "line-2".into(),
                bytes: Bytes::Public(ended.to_vec()),
            },
            Row {
                store: StoreKind::LeaseRecords,
                identity: "line-3".into(),
                bytes: Bytes::Public(granted.to_vec()),
            },
        ],
        vec![],
        None,
    )
    .await;
    let place = roots();
    let audit = MemoryAudit::open();
    let report = stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    assert_eq!(report.leases, vec!["lease-again".to_owned()]);
    assert_eq!(
        fs::read(place.quarantine.join("records/lease-records/line-1")).unwrap(),
        granted
    );
    let last = fs::read(place.quarantine.join("records/lease-records/line-3")).unwrap();
    let text = String::from_utf8(last).unwrap();
    assert!(text.contains("\"kind\":\"granted\""));
    assert!(text.contains("\"kind\":\"ended\""));
    let error = accept(&place, &audit, &NoReceipts).unwrap_err();
    assert_eq!(error, RestoreFault::ExecutorsUnfenced);
}

#[tokio::test]
async fn a_fenced_deferred_conversation_is_not_listed() {
    let digest = Fingerprint::of(b"session-v1");
    let log = DeletionLog::default()
        .append(DeletionFence::conversation("conv-bulk").unwrap())
        .0;
    let bundle = publish(
        note_bundle_rows(),
        deferred_rows("conv-bulk", 11, digest),
        Some(log),
    )
    .await;
    let place = roots();
    let audit = MemoryAudit::open();
    stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    let text = fs::read_to_string(place.quarantine.join("deferred.txt")).unwrap();
    assert!(!text.contains("conv-bulk"));
    accept(&place, &audit, &NoReceipts).unwrap();
    let source = FakeSource::unused();
    let error = open_one(&place, "conv-bulk", &source, &audit).unwrap_err();
    assert_eq!(error, RestoreFault::NotDeferred);
    assert_eq!(*source.writes.lock().unwrap(), 0);
}

#[tokio::test]
async fn a_changed_session_file_is_not_promoted() {
    let digest = Fingerprint::of(b"session-v1");
    let bundle = publish(
        note_bundle_rows(),
        deferred_rows("conv-bulk", 11, digest),
        None,
    )
    .await;
    let place = roots();
    let audit = MemoryAudit::open();
    stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    let session = place.quarantine.join("sessions/conv-bulk.sqlite3");
    nessa_local_storage::create_directory(session.parent().unwrap()).unwrap();
    fs::write(&session, b"not-the-session").unwrap();
    let error = accept(&place, &audit, &NoReceipts).unwrap_err();
    assert_eq!(error, RestoreFault::Tamper);
    assert!(!place.live.exists());
}

#[tokio::test]
async fn an_unfenced_executor_blocks_acceptance() {
    let bundle = publish(
        vec![Row {
            store: StoreKind::LeaseRecords,
            identity: "line-1".into(),
            bytes: Bytes::Public(br#"{"kind":"command_granted","lease":"lease-9"}"#.to_vec()),
        }],
        vec![],
        None,
    )
    .await;
    let place = roots();
    let audit = MemoryAudit::open();
    stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    let error = accept(&place, &audit, &MapReceipts(&[("lease-9", "bad\nreceipt")])).unwrap_err();
    assert_eq!(error, RestoreFault::ExecutorsUnfenced);
    assert!(!place.live.exists());
    let error = accept(&place, &audit, &MapReceipts(&[("lease-9", "")])).unwrap_err();
    assert_eq!(error, RestoreFault::ExecutorsUnfenced);
    accept(&place, &audit, &MapReceipts(&[("lease-9", "host-stopped")])).unwrap();
    let expected_cause = if cfg!(windows) {
        "unsynced"
    } else {
        "accepted"
    };
    let accepted = audit
        .lines()
        .into_iter()
        .find(|line| line.cause == expected_cause)
        .unwrap();
    assert_eq!(accepted.attestations, "lease-9 host-stopped\n");
    assert_eq!(accepted.after, "live");
}

struct SealedSession {
    bytes: Vec<u8>,
    size: u64,
    digest: Fingerprint,
}

fn sealed_session(payload: &[u8]) -> SealedSession {
    let bytes = session_database(&[("conv-bulk", payload)]);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.sqlite3");
    fs::write(&path, &bytes).unwrap();
    let cut = DirectoryQuarantine.session_cut(&path, "conv-bulk").unwrap();
    SealedSession {
        bytes,
        size: cut.size,
        digest: cut.digest,
    }
}

struct FakeSource {
    bytes: Vec<u8>,
    other: Vec<u8>,
    mode: &'static str,
    writes: Mutex<u32>,
}

impl FakeSource {
    fn matching(session: &SealedSession) -> Self {
        Self {
            bytes: session.bytes.clone(),
            other: sealed_session(b"other-payload").bytes,
            mode: "match",
            writes: Mutex::new(0),
        }
    }

    fn unused() -> Self {
        Self {
            bytes: Vec::new(),
            other: Vec::new(),
            mode: "match",
            writes: Mutex::new(0),
        }
    }
}

impl DeferredSessionSource for FakeSource {
    fn write_session(
        &self,
        _conversation_id: &str,
        destination: &mut fs::File,
        progress: &mut dyn FetchProgress,
    ) -> Result<(), SessionSourceError> {
        *self.writes.lock().unwrap() += 1;
        progress.report(0, self.bytes.len() as u64);
        match self.mode {
            "missing" => Err(SessionSourceError::Missing),
            "interrupted" => {
                destination.write_all(b"partial-session").unwrap();
                Err(SessionSourceError::Interrupted)
            }
            "changed" => {
                destination.write_all(&self.other).unwrap();
                Ok(())
            }
            _ => {
                destination.write_all(&self.bytes).unwrap();
                Ok(())
            }
        }
    }
}

fn deferred_rows(conversation_id: &str, size: u64, digest: Fingerprint) -> Vec<DeferredEntry> {
    vec![
        DeferredEntry::new(StoreKind::CanonicalRecords, conversation_id, size, digest).unwrap(),
        DeferredEntry::new(StoreKind::Notes, conversation_id, size, digest).unwrap(),
    ]
}

async fn deferred_bundle(lines: Vec<DeferredEntry>) -> (Bundle, Roots, MemoryAudit) {
    let bundle = publish(note_bundle_rows(), lines, None).await;
    let place = roots();
    let audit = MemoryAudit::open();
    stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    accept(&place, &audit, &NoReceipts).unwrap();
    (bundle, place, audit.checkpoint())
}

fn mark_available(audit: &MemoryAudit, root: &Path, conversation_id: &str) {
    let anchored = DirectoryQuarantine.anchor(root).unwrap();
    let target = format!("{conversation_id}\n{}", anchored.display());
    let publication = "pub-current";
    let session = anchored.join(format!("sessions/{conversation_id}.sqlite3"));
    fixture_note_publication(&session, publication);
    audit
        .append(&RestoreEvidence {
            target: &target,
            before: "held",
            after: "available",
            cause: "fetched",
            initiator: "person",
            decision: "effect",
            policies: &[],
            attestations: publication,
            journal_digest: "",
            root_generation: "",
        })
        .unwrap();
}

fn open_one(
    place: &Roots,
    conversation_id: &str,
    source: &dyn DeferredSessionSource,
    audit: &dyn RestoreAudit,
) -> Result<OpenReport, RestoreFault> {
    open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &place.live,
            conversation_id,
        },
        &policy(),
        audit,
        &DirectoryQuarantine,
        source,
        &OsPublicationIds,
        &mut Quiet,
    )
}

fn replace_deferred_session(root: &Path, session: &SealedSession) {
    let deferred_path = root.join("deferred.txt");
    let mut lines =
        crate::backup::domain::decode_deferred(&fs::read_to_string(&deferred_path).unwrap())
            .unwrap();
    for line in &mut lines {
        line.size = session.size;
        line.digest = session.digest;
    }
    let deferred = crate::backup::domain::encode_deferred(&lines).unwrap();
    fs::write(&deferred_path, &deferred).unwrap();
    let journal_path = root.join("journal.txt");
    let journal = read_journal(&fs::read_to_string(&journal_path).unwrap()).unwrap();
    let mut files = journal.files;
    let sealed = files
        .iter_mut()
        .find(|file| file.path == "deferred.txt")
        .unwrap();
    sealed.fingerprint = Fingerprint::of(deferred.as_bytes());
    fs::write(
        journal_path,
        encode_journal(journal.freshness, &journal.leases, &files).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn a_resealed_deferred_line_is_not_fetched_in_quarantine() {
    let original = sealed_session(b"session-v1");
    let replacement = sealed_session(b"session-v2");
    assert_ne!(original.digest, replacement.digest);
    let bundle = publish(
        note_bundle_rows(),
        deferred_rows("conv-bulk", original.size, original.digest),
        None,
    )
    .await;
    let place = roots();
    let audit = MemoryAudit::open();
    stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    replace_deferred_session(&place.quarantine, &replacement);
    let source = FakeSource::matching(&replacement);
    let result = open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &place.quarantine,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &audit,
        &DirectoryQuarantine,
        &source,
        &OsPublicationIds,
        &mut Quiet,
    );
    assert_eq!(result.unwrap_err(), RestoreFault::Tamper);
    assert_eq!(*source.writes.lock().unwrap(), 0);
    assert!(!place.quarantine.join("sessions/conv-bulk.sqlite3").exists());
    assert!(!audit.lines().iter().any(|line| line.after == "available"));
}

#[tokio::test]
async fn a_resealed_deferred_line_is_not_fetched_after_acceptance() {
    let original = sealed_session(b"session-v1");
    let replacement = sealed_session(b"session-v2");
    assert_ne!(original.digest, replacement.digest);
    let bundle = publish(
        note_bundle_rows(),
        deferred_rows("conv-bulk", original.size, original.digest),
        None,
    )
    .await;
    let place = roots();
    let audit = MemoryAudit::open();
    stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    accept(&place, &audit, &NoReceipts).unwrap();
    replace_deferred_session(&place.live, &replacement);
    let source = FakeSource::matching(&replacement);
    assert_eq!(
        open_one(&place, "conv-bulk", &source, &audit).unwrap_err(),
        RestoreFault::Tamper,
    );
    assert_eq!(*source.writes.lock().unwrap(), 0);
    assert!(!place.live.join("sessions/conv-bulk.sqlite3").exists());
    assert!(!audit.lines().iter().any(|line| line.after == "available"));
}

#[tokio::test]
async fn an_opened_deferred_conversation_matches_the_recorded_fingerprint() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place, audit) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let source = FakeSource::matching(&session);
    let report = open_one(&place, "conv-bulk", &source, &audit).unwrap();
    assert_eq!(report.size, session.size);
    assert_eq!(report.digest, session.digest);
    assert!(!report.already_present);
    assert_eq!(*source.writes.lock().unwrap(), 1);
    assert!(place.live.join("sessions/conv-bulk.sqlite3").is_file());
    let lines = audit.lines();
    let matched = lines.iter().find(|line| line.cause == "matched").unwrap();
    assert_eq!(matched.before, "deferred");
    assert_eq!(matched.after, "held");
    let fetched = lines.iter().find(|line| line.cause == "fetched").unwrap();
    assert_eq!(fetched.before, "held");
    assert_eq!(fetched.after, "available");
    assert_eq!(fetched.initiator, "person");
    assert_eq!(fetched.decision, "effect");
    let anchored = DirectoryQuarantine.anchor(&place.live).unwrap();
    let target = format!("conv-bulk\n{}", anchored.display());
    assert_eq!(fetched.target, target);
    assert_eq!(matched.target, target);
    assert!(fetched.attestations.starts_with("pub-"));
    assert_eq!(fetched.attestations.len(), 4 + 32);
    assert!(fetched.attestations[4..]
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(
        DirectoryQuarantine
            .publication(&place.live.join("sessions/conv-bulk.sqlite3"))
            .unwrap()
            .as_deref(),
        Some(fetched.attestations.as_str())
    );
}

struct FixedIds(&'static str);

impl PublicationIds for FixedIds {
    fn publication_id(&self) -> Result<String, RestoreFault> {
        Ok(self.0.to_owned())
    }
}

struct NoIds;

impl PublicationIds for NoIds {
    fn publication_id(&self) -> Result<String, RestoreFault> {
        Err(RestoreFault::Unreadable)
    }
}

#[tokio::test]
async fn a_port_supplied_publication_id_is_the_one_the_audit_names() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place, audit) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let source = FakeSource::matching(&session);
    open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &place.live,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &audit,
        &DirectoryQuarantine,
        &source,
        &FixedIds("pub-fixed"),
        &mut Quiet,
    )
    .unwrap();
    let fetched = audit
        .lines()
        .into_iter()
        .find(|line| line.cause == "fetched")
        .unwrap();
    assert_eq!(fetched.attestations, "pub-fixed");
    assert_eq!(
        DirectoryQuarantine
            .publication(&place.live.join("sessions/conv-bulk.sqlite3"))
            .unwrap()
            .as_deref(),
        Some("pub-fixed")
    );
}

#[tokio::test]
async fn a_failed_publication_id_retires_the_published_session() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place, audit) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let source = FakeSource::matching(&session);
    let error = open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &place.live,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &audit,
        &DirectoryQuarantine,
        &source,
        &NoIds,
        &mut Quiet,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Retained);
    assert_eq!(
        fs::metadata(place.live.join("sessions/conv-bulk.sqlite3"))
            .unwrap()
            .len(),
        0
    );
    assert!(DirectoryQuarantine
        .publication(&place.live.join("sessions/conv-bulk.sqlite3"))
        .unwrap()
        .is_none());
    assert!(partial_names(&place.live).is_empty());
}

#[tokio::test]
async fn a_changed_deferred_line_is_not_fetched() {
    let digest = Fingerprint::of(b"session-v1");
    let (_bundle, place, audit) = deferred_bundle(deferred_rows("conv-bulk", 11, digest)).await;
    let path = place.live.join("deferred.txt");
    let mut text = fs::read_to_string(&path).unwrap();
    text = text.replace("11", "12");
    fs::write(&path, text).unwrap();
    let source = FakeSource::unused();
    let error = open_one(&place, "conv-bulk", &source, &audit).unwrap_err();
    assert_eq!(error, RestoreFault::Tamper);
    assert_eq!(*source.writes.lock().unwrap(), 0);
    assert!(!place.live.join("sessions/conv-bulk.sqlite3").exists());
}

#[tokio::test]
async fn a_failed_fetch_audit_removes_the_session() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place, audit) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let source = FakeSource::matching(&session);
    let error = open_one(&place, "conv-bulk", &source, &audit.failing_after(1)).unwrap_err();
    assert_eq!(error, RestoreFault::Audit);
    assert!(!place.live.join("sessions/conv-bulk.sqlite3").exists());
    assert!(partial_names(&place.live).is_empty());
}

#[tokio::test]
async fn a_failed_fetch_cleanup_moves_the_session() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place, audit) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let source = FakeSource::matching(&session);
    let error = open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &place.live,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &audit.failing_after(2),
        &KeepsSession,
        &source,
        &OsPublicationIds,
        &mut Quiet,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Retained);
    assert!(!place.live.join("sessions/conv-bulk.sqlite3").exists());
    assert!(place
        .live
        .join("sessions/conv-bulk.sqlite3.rejected")
        .is_file());
}

#[tokio::test]
async fn a_changed_source_is_refused_and_other_conversations_stay() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place, audit) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let other = place.live.join("records/notes/note-1");
    let before = fs::read(&other).unwrap();
    let mut source = FakeSource::matching(&session);
    source.mode = "changed";
    let error = open_one(&place, "conv-bulk", &source, &audit).unwrap_err();
    assert_eq!(error, RestoreFault::SourceChanged);
    assert!(!place.live.join("sessions/conv-bulk.sqlite3").is_file());
    assert!(partial_names(&place.live).is_empty());
    assert_eq!(fs::read(&other).unwrap(), before);
}

#[tokio::test]
async fn a_missing_source_is_refused_and_other_conversations_stay() {
    let digest = Fingerprint::of(b"session-v1");
    let (_bundle, place, audit) = deferred_bundle(deferred_rows("conv-bulk", 11, digest)).await;
    let other = place.live.join("records/notes/note-1");
    let before = fs::read(&other).unwrap();
    let mut source = FakeSource::unused();
    source.mode = "missing";
    let error = open_one(&place, "conv-bulk", &source, &audit).unwrap_err();
    assert_eq!(error, RestoreFault::SourceMissing);
    assert_eq!(fs::read(&other).unwrap(), before);
    assert!(!place.live.join("sessions/conv-bulk.sqlite3").exists());
}

#[tokio::test]
async fn an_interrupted_fetch_leaves_no_partial_session() {
    let digest = Fingerprint::of(b"session-v1");
    let (_bundle, place, audit) = deferred_bundle(deferred_rows("conv-bulk", 11, digest)).await;
    let mut source = FakeSource::unused();
    source.mode = "interrupted";
    let error = open_one(&place, "conv-bulk", &source, &audit).unwrap_err();
    assert_eq!(error, RestoreFault::Interrupted);
    assert!(partial_names(&place.live).is_empty());
    assert!(!place.live.join("sessions/conv-bulk.sqlite3").exists());
    assert!(!place
        .live
        .join("sessions/conv-bulk.sqlite3.partial")
        .exists());
}

#[tokio::test]
async fn an_interrupted_fetch_that_cannot_be_removed_is_retained() {
    let digest = Fingerprint::of(b"session-v1");
    let (_bundle, place, audit) = deferred_bundle(deferred_rows("conv-bulk", 11, digest)).await;
    let mut source = FakeSource::unused();
    source.mode = "interrupted";
    let error = open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &place.live,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &audit,
        &KeepsPartialSession,
        &source,
        &OsPublicationIds,
        &mut Quiet,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Retained);
    assert!(place
        .live
        .join("sessions/conv-bulk.sqlite3.partial")
        .is_file());
    assert!(audit.lines().iter().all(|line| line.after != "unchanged"));
    let later = open_one(&place, "conv-bulk", &source, &audit).unwrap_err();
    assert_eq!(later, RestoreFault::Tamper);
}

#[cfg(unix)]
#[tokio::test]
async fn a_non_unicode_root_is_not_audited() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    let quarantine = place
        ._keep
        .path()
        .join(OsString::from_vec(vec![0xff, 0xfe]));
    let audit = MemoryAudit::open();
    let error = stage(
        &bundle,
        &quarantine,
        &place.live,
        &audit,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Paths);
    assert!(audit.lines().is_empty());
    assert!(!place.live.exists());
}

/// Removal, the move aside, and emptying all fail for a `.partial` session.
struct KeepsPartialSession;

impl QuarantineLayout for KeepsPartialSession {
    fn begin_session(
        &self,
        root: &AnchoredRoot,
        relative: &str,
    ) -> Result<Box<dyn SessionAttempt + '_>, RestoreFault> {
        TestAttempt::begin(TestRetirement::RetainPartial, root, relative)
    }

    fn begin(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
    ) -> Result<Box<dyn Staging>, RestoreFault> {
        DirectoryQuarantine.begin(quarantine, live)
    }
    fn load(&self, root: &AnchoredRoot) -> Result<LoadedTree, RestoreFault> {
        DirectoryQuarantine.load(root)
    }
    fn promote(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
        loaded: &LoadedTree,
        generation: &str,
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded, generation)
    }
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }

    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }

    fn exists(&self, path: &Path) -> Result<bool, RestoreFault> {
        DirectoryQuarantine.exists(path)
    }
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.canonicalize(path)
    }
    fn anchor(&self, path: &Path) -> Result<AnchoredRoot, RestoreFault> {
        DirectoryQuarantine.anchor(path)
    }
    fn hold(&self, root: &AnchoredRoot) -> Result<Box<dyn RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }
    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }

    fn session_cut(&self, path: &Path, conversation_id: &str) -> Result<SessionCut, RestoreFault> {
        DirectoryQuarantine.session_cut(path, conversation_id)
    }
}

struct LeavesPartial {
    partial: PathBuf,
}

impl Staging for LeavesPartial {
    fn write(&mut self, _relative: &str, _bytes: &[u8]) -> Result<(), RestoreFault> {
        fs::create_dir_all(&self.partial).unwrap();
        fs::write(self.partial.join("kept"), b"partial").unwrap();
        Err(RestoreFault::Unreadable)
    }

    fn finish(self: Box<Self>, _journal: &str) -> Result<Box<dyn StagedTreeOwner>, RestoreFault> {
        Err(RestoreFault::Unreadable)
    }

    fn discard(self: Box<Self>) -> Result<(), RestoreFault> {
        Err(RestoreFault::Retained)
    }
}

struct LeavesPartialTree {
    partial: PathBuf,
}

impl QuarantineLayout for LeavesPartialTree {
    fn begin_session(
        &self,
        root: &AnchoredRoot,
        relative: &str,
    ) -> Result<Box<dyn SessionAttempt + '_>, RestoreFault> {
        DirectoryQuarantine.begin_session(root, relative)
    }

    fn begin(
        &self,
        _quarantine: &AnchoredRoot,
        _live: &AnchoredRoot,
    ) -> Result<Box<dyn Staging>, RestoreFault> {
        Ok(Box::new(LeavesPartial {
            partial: self.partial.clone(),
        }))
    }
    fn load(&self, root: &AnchoredRoot) -> Result<LoadedTree, RestoreFault> {
        DirectoryQuarantine.load(root)
    }
    fn promote(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
        loaded: &LoadedTree,
        generation: &str,
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded, generation)
    }
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }

    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }

    fn exists(&self, path: &Path) -> Result<bool, RestoreFault> {
        DirectoryQuarantine.exists(path)
    }
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.canonicalize(path)
    }
    fn anchor(&self, path: &Path) -> Result<AnchoredRoot, RestoreFault> {
        DirectoryQuarantine.anchor(path)
    }
    fn hold(&self, root: &AnchoredRoot) -> Result<Box<dyn RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }
    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }

    fn session_cut(&self, path: &Path, conversation_id: &str) -> Result<SessionCut, RestoreFault> {
        DirectoryQuarantine.session_cut(path, conversation_id)
    }
}

#[tokio::test]
async fn a_partial_directory_that_survives_a_failed_write_is_unready() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    let partial = place._keep.path().join("left-behind");
    let source = DirectoryVerificationSource::open(bundle.root.clone()).unwrap();
    let audit = MemoryAudit::open();
    let error = stage_quarantine(
        &StageRequest {
            facts: person(RestoreAction::Stage),
            bundle: &source,
            format: &ExportFormat,
            verifier: &bundle.key,
            limits: VerificationLimits::default(),
            later_inventory: None,
            quarantine: &place.quarantine,
            live: &place.live,
        },
        &policy(),
        &audit,
        &LeavesPartialTree {
            partial: partial.clone(),
        },
        &DirectoryMaterializer,
        &KeyedInventory::new(bundle.key),
        &OsStagingIds,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Retained);
    assert_eq!(fs::read(partial.join("kept")).unwrap(), b"partial");
    assert!(audit.lines().iter().any(|line| {
        line.before == "absent" && line.after == "unready" && line.cause == "unreadable"
    }));
    assert!(audit.lines().iter().all(|line| line.after != "unchanged"));
    assert!(!place.live.exists());
    assert!(!place.quarantine.exists());
}

#[tokio::test]
async fn an_unknown_conversation_is_not_fetched() {
    let digest = Fingerprint::of(b"session-v1");
    let (_bundle, place, audit) = deferred_bundle(deferred_rows("conv-bulk", 11, digest)).await;
    let source = FakeSource::unused();
    let error = open_one(&place, "conv-other", &source, &audit).unwrap_err();
    assert_eq!(error, RestoreFault::NotDeferred);
    assert_eq!(*source.writes.lock().unwrap(), 0);
}

#[tokio::test]
async fn disagreeing_deferred_lines_are_refused() {
    let lines = vec![
        DeferredEntry::new(
            StoreKind::CanonicalRecords,
            "conv-bulk",
            11,
            Fingerprint::of(b"one"),
        )
        .unwrap(),
        DeferredEntry::new(StoreKind::Notes, "conv-bulk", 12, Fingerprint::of(b"two")).unwrap(),
    ];
    let (_bundle, place, audit) = deferred_bundle(lines).await;
    let source = FakeSource::unused();
    let error = open_one(&place, "conv-bulk", &source, &audit).unwrap_err();
    assert_eq!(error, RestoreFault::Fingerprint);
    assert_eq!(*source.writes.lock().unwrap(), 0);
}

#[tokio::test]
async fn a_matching_session_is_not_fetched_again() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place, audit) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let path = place.live.join("sessions/conv-bulk.sqlite3");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, &session.bytes).unwrap();
    let source = FakeSource::matching(&session);
    mark_available(&audit, &place.live, "conv-bulk");
    let report = open_one(&place, "conv-bulk", &source, &audit).unwrap();
    assert!(report.already_present);
    assert_eq!(*source.writes.lock().unwrap(), 0);
    assert_eq!(fs::read(&path).unwrap(), session.bytes);
}

#[tokio::test]
async fn a_different_session_file_is_not_replaced() {
    let session = sealed_session(b"session-v1");
    let other = sealed_session(b"other-payload");
    let (_bundle, place, audit) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let path = place.live.join("sessions/conv-bulk.sqlite3");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, &other.bytes).unwrap();
    let source = FakeSource::matching(&session);
    mark_available(&audit, &place.live, "conv-bulk");
    let error = open_one(&place, "conv-bulk", &source, &audit).unwrap_err();
    assert_eq!(error, RestoreFault::SessionDiffers);
    assert_eq!(*source.writes.lock().unwrap(), 0);
    assert_eq!(fs::read(&path).unwrap(), other.bytes);
}

#[tokio::test]
async fn a_session_over_64_mib_is_restored_when_opened() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let sessions = root.join("conversations/sessions");
    fs::create_dir_all(&sessions).unwrap();
    fs::create_dir_all(root.join("environment")).unwrap();
    fs::write(
        root.join("environment/leases.jsonl"),
        b"{\"kind\":\"granted\",\"lease\":\"lease-drill\"}\n",
    )
    .unwrap();
    let kept = b"SMALL-CONVERSATION-STAYS-NAMED";
    let kept_incarnation = [0x11u8; 16];
    let bulk_incarnation = [0x22u8; 16];
    let tail = [0u8, 0, 0, 0, 0, 0, 0, 1];
    let offset = [0u8, 0, 0, 0, 0, 0, 0, 1];
    let bulk_len = usize::try_from(SESSION_EXPORT_LIMIT).unwrap() + 4096;
    let database_path = sessions.join("records.sqlite3");
    let database = nessa_local_database::rusqlite::Connection::open(&database_path).unwrap();
    database
        .execute_batch(
            "CREATE TABLE event_streams(
                stream_key INTEGER PRIMARY KEY,
                public_id TEXT NOT NULL,
                incarnation BLOB NOT NULL,
                floor BLOB NOT NULL,
                tail BLOB NOT NULL,
                retired INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE event_stream_names(
                public_id TEXT PRIMARY KEY,
                latest_incarnation BLOB NOT NULL,
                active_stream_key INTEGER
            );
            CREATE TABLE event_records(
                stream_key INTEGER NOT NULL,
                offset BLOB NOT NULL,
                event_id TEXT NOT NULL,
                schema_id TEXT NOT NULL,
                schema_version INTEGER NOT NULL,
                payload BLOB NOT NULL
            );",
        )
        .unwrap();
    database
        .execute(
            "INSERT INTO event_streams(stream_key, public_id, incarnation, floor, tail)
             VALUES (1, 'conv-kept', ?1, ?2, ?2)",
            [kept_incarnation.as_slice(), tail.as_slice()],
        )
        .unwrap();
    database
        .execute(
            "INSERT INTO event_streams(stream_key, public_id, incarnation, floor, tail)
             VALUES (2, 'conv-bulk', ?1, ?2, ?2)",
            [bulk_incarnation.as_slice(), tail.as_slice()],
        )
        .unwrap();
    database
        .execute(
            "INSERT INTO event_stream_names(public_id, latest_incarnation, active_stream_key)
             VALUES ('conv-kept', ?1, 1), ('conv-bulk', ?2, 2)",
            [kept_incarnation.as_slice(), bulk_incarnation.as_slice()],
        )
        .unwrap();
    database
        .execute(
            "INSERT INTO event_records(stream_key, offset, event_id, schema_id, schema_version, payload)
             VALUES (1, ?1, 'kept-event', 'nessa.record', 1, ?2)",
            [offset.as_slice(), kept.as_slice()],
        )
        .unwrap();
    database
        .execute(
            "INSERT INTO event_records(stream_key, offset, event_id, schema_id, schema_version, payload)
             VALUES (2, ?1, 'bulk-event', 'nessa.record', 1, zeroblob(?2))",
            nessa_local_database::rusqlite::params![offset.as_slice(), bulk_len as i64],
        )
        .unwrap();
    drop(database);

    let inventory = FileDeletionLog::open(root.join("backup")).unwrap();
    let published = tempfile::tempdir().unwrap();
    capture_namespace(
        root,
        &OpenAdmission,
        &inventory,
        &DirectoryPublisher::new(published.path().join("cut")),
        &Clock(8),
        &inventory.key(),
    )
    .await
    .unwrap();
    let bundle_root = published.path().join("cut");
    let manifest = fs::read_to_string(bundle_root.join("manifest.txt")).unwrap();
    assert!(manifest.contains(&format!("deferred canonical-records conv-bulk {bulk_len} ")));
    assert!(!bundle_root
        .join("records/canonical-records/records.sqlite3")
        .exists());
    assert!(!manifest.contains("oversized"));

    let place = roots();
    let audit_dir = place._keep.path().join("audit");
    let audit = FileRestoreAudit::open(&audit_dir, Clock(42)).unwrap();
    let bundle = Bundle {
        _keep: published,
        root: bundle_root,
        key: inventory.key(),
    };
    stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    assert_eq!(
        root_posture(&place.quarantine, &place.live, &DirectoryQuarantine).unwrap(),
        Posture::Quarantined
    );
    assert!(!holds(&place.quarantine, kept));
    let blocked = accept(&place, &audit, &NoReceipts).unwrap_err();
    assert_eq!(blocked, RestoreFault::ExecutorsUnfenced);
    assert!(!place.live.exists());
    accept(
        &place,
        &audit,
        &MapReceipts(&[("lease-drill", "stopped-host-1")]),
    )
    .unwrap();
    assert_eq!(
        fs::read(place.live.join("replay.blocked")).unwrap(),
        b"blocked\n"
    );
    assert_eq!(
        fs::read(place.live.join("identity.txt")).unwrap(),
        b"re-pair\n"
    );
    assert_eq!(
        root_posture(&place.live, &place.live, &DirectoryQuarantine).unwrap(),
        Posture::Live
    );
    let restored_lease =
        fs::read_to_string(place.live.join("records/lease-records/line-1")).unwrap();
    assert!(restored_lease.contains("\"outcome\":\"uncertain\""));

    let source = SqliteDeferredSource::new(&database_path);
    let opened = open_one(&place, "conv-bulk", &source, &audit).unwrap();
    #[cfg(windows)]
    assert_eq!(opened.durable, Some(false));
    assert!(!opened.already_present);
    assert_eq!(opened.size, bulk_len as u64);
    let session = place.live.join("sessions/conv-bulk.sqlite3");
    let inspected = DirectoryQuarantine
        .session_cut(&session, "conv-bulk")
        .unwrap();
    assert_eq!(inspected.size, opened.size);
    assert_eq!(inspected.digest, opened.digest);
    let deferred = fs::read_to_string(place.live.join("deferred.txt")).unwrap();
    assert!(deferred.contains(&format!(
        "canonical-records conv-bulk {} {}",
        opened.size,
        opened.digest.hex()
    )));
    assert!(!holds(&place.live, kept));

    fs::remove_file(&database_path).unwrap();
    let missing = open_one(&place, "conv-kept", &source, &audit).unwrap_err();
    assert_eq!(missing, RestoreFault::SourceMissing);
    let again = DirectoryQuarantine
        .session_cut(&session, "conv-bulk")
        .unwrap();
    assert_eq!(again, inspected);
    assert!(!place.live.join("sessions/conv-kept.sqlite3").exists());

    let record = fs::read_to_string(audit_dir.join("restore.jsonl")).unwrap();
    assert!(record.contains("\"cause\":\"sealed\""));
    let expected_cause = if cfg!(windows) {
        "unsynced"
    } else {
        "accepted"
    };
    let live_effects: Vec<serde_json::Value> = record
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .filter(|line: &serde_json::Value| line["after"] == "live")
        .collect();
    assert_eq!(live_effects.len(), 1);
    assert_eq!(live_effects[0]["cause"], expected_cause);
    assert!(record.contains("\"cause\":\"fetched\""));
    assert!(record.contains("\"initiator\":\"person\""));
    assert!(record.contains("\"observedAtMs\":42"));
    assert!(record.contains("\"recordId\":\"restore-1\""));
    assert!(record.contains("\"before\":\"absent\""));
    assert!(record.contains("\"after\":\"verified\""));
}

#[tokio::test]
async fn a_fenced_conversation_inside_the_session_is_not_restored() {
    let kept = b"KEPT-STREAM-BYTES";
    let fenced = b"FENCED-STREAM-BYTES";
    let bytes = session_database(&[("conv-kept", kept), ("conv-fenced", fenced)]);
    let later = DeletionLog::default()
        .append(DeletionFence::conversation("conv-fenced").unwrap())
        .0;
    let bundle = publish(
        vec![Row {
            store: StoreKind::CanonicalRecords,
            identity: "records.sqlite3".into(),
            bytes: Bytes::Public(bytes),
        }],
        vec![],
        None,
    )
    .await;
    let sealed = seal_inventory(bundle.key, &later);
    let place = roots();
    stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &MemoryAudit::open(),
        person(RestoreAction::Stage),
        Some(&sealed),
    )
    .unwrap();
    assert!(holds(&place.quarantine, kept));
    assert!(!holds(&place.quarantine, fenced));
    assert!(!holds(&place.quarantine, b"conv-fenced"));
    assert!(!place.live.exists());

    let absent = DeletionLog::default()
        .append(DeletionFence::conversation("conv-other").unwrap())
        .0;
    let bytes = session_database(&[("conv-kept", kept)]);
    let bundle = publish(
        vec![Row {
            store: StoreKind::CanonicalRecords,
            identity: "records.sqlite3".into(),
            bytes: Bytes::Public(bytes),
        }],
        vec![],
        None,
    )
    .await;
    let sealed = seal_inventory(bundle.key, &absent);
    let place = roots();
    stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &MemoryAudit::open(),
        person(RestoreAction::Stage),
        Some(&sealed),
    )
    .unwrap();
    assert!(holds(&place.quarantine, kept));
}

fn session_database(rows: &[(&str, &[u8])]) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("records.sqlite3");
    let database = nessa_local_database::rusqlite::Connection::open(&path).unwrap();
    database
        .execute_batch(
            "CREATE TABLE event_streams(
                stream_key INTEGER PRIMARY KEY,
                public_id TEXT NOT NULL,
                incarnation BLOB NOT NULL,
                floor BLOB NOT NULL,
                tail BLOB NOT NULL,
                retired INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE event_stream_names(
                public_id TEXT PRIMARY KEY,
                latest_incarnation BLOB NOT NULL,
                active_stream_key INTEGER
            );
            CREATE TABLE event_records(
                stream_key INTEGER NOT NULL,
                offset BLOB NOT NULL,
                event_id TEXT NOT NULL,
                schema_id TEXT NOT NULL,
                schema_version INTEGER NOT NULL,
                payload BLOB NOT NULL
            );",
        )
        .unwrap();
    for (index, (id, payload)) in rows.iter().enumerate() {
        let key = index as i64 + 1;
        let incarnation = [u8::try_from(index).unwrap().saturating_add(1); 16];
        let tail = [0u8, 0, 0, 0, 0, 0, 0, 1];
        database
            .execute(
                "INSERT INTO event_streams(stream_key, public_id, incarnation, floor, tail)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                nessa_local_database::rusqlite::params![
                    key,
                    id,
                    incarnation.as_slice(),
                    tail.as_slice()
                ],
            )
            .unwrap();
        database
            .execute(
                "INSERT INTO event_stream_names(public_id, latest_incarnation, active_stream_key)
                 VALUES (?1, ?2, ?3)",
                nessa_local_database::rusqlite::params![id, incarnation.as_slice(), key],
            )
            .unwrap();
        database
            .execute(
                "INSERT INTO event_records(stream_key, offset, event_id, schema_id, schema_version, payload)
                 VALUES (?1, ?2, 'event', 'nessa.record', 1, ?3)",
                nessa_local_database::rusqlite::params![key, tail.as_slice(), payload],
            )
            .unwrap();
    }
    drop(database);
    fs::read(path).unwrap()
}

#[tokio::test]
async fn a_lost_refusal_audit_is_not_a_plain_refusal() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    let audit = MemoryAudit::fail_after(1);
    let error = stage(
        &bundle,
        &place.live,
        &place.live,
        &audit,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Audit);
    assert_eq!(audit.lines().len(), 1);
    assert_eq!(audit.lines()[0].decision, "allow");
    let recorded = MemoryAudit::open();
    let error = stage(
        &bundle,
        &place.live,
        &place.live,
        &recorded,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Paths);
    assert!(recorded
        .lines()
        .iter()
        .any(|line| line.cause == "paths" && line.decision == "effect"));
    assert!(!place.live.exists());
}

#[tokio::test]
async fn accept_and_fetch_wait_for_the_restore_root() {
    use std::sync::mpsc;
    use std::time::Duration;

    let session = sealed_session(b"session");
    let bundle = publish(
        note_bundle_rows(),
        deferred_rows("conv-bulk", session.size, session.digest),
        None,
    )
    .await;
    let place = roots();
    stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &MemoryAudit::open(),
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    let quarantine = DirectoryQuarantine.anchor(&place.quarantine).unwrap();
    let live = DirectoryQuarantine.anchor(&place.live).unwrap();
    let hold = DirectoryQuarantine.hold(&quarantine).unwrap();
    let (fetch_started, fetch_ready) = mpsc::channel();
    let (fetch_finished, fetch_result) = mpsc::channel();
    let fetch_root = quarantine.path().to_path_buf();
    std::thread::spawn(move || {
        fetch_started.send(()).unwrap();
        let outcome = open_deferred_conversation(
            &OpenRequest {
                facts: person(RestoreAction::Fetch),
                root: &fetch_root,
                conversation_id: "conv-bulk",
            },
            &policy(),
            &MemoryAudit::open(),
            &DirectoryQuarantine,
            &FakeSource::matching(&session),
            &OsPublicationIds,
            &mut Quiet,
        );
        fetch_finished.send(outcome.map(|_| ())).unwrap();
    });
    let (accept_started, accept_ready) = mpsc::channel();
    let (accept_finished, accept_result) = mpsc::channel();
    let accept_root = quarantine.path().to_path_buf();
    let accept_live = live.path().to_path_buf();
    std::thread::spawn(move || {
        accept_started.send(()).unwrap();
        let outcome = accept_quarantine(
            &AcceptRequest {
                facts: person(RestoreAction::Accept),
                quarantine: &accept_root,
                live: &accept_live,
                receipts: &NoReceipts,
            },
            &policy(),
            &MemoryAudit::open(),
            &DirectoryQuarantine,
        );
        accept_finished.send(outcome.map(|_| ())).unwrap();
    });
    fetch_ready.recv_timeout(Duration::from_secs(10)).unwrap();
    accept_ready.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(
        matches!(
            fetch_result.recv_timeout(Duration::from_secs(1)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ),
        "fetch must wait for the restore-root lock"
    );
    assert!(
        matches!(
            accept_result.recv_timeout(Duration::from_secs(1)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ),
        "accept must wait for the restore-root lock"
    );
    assert!(!live.exists());
    drop(hold);
    let fetched = fetch_result.recv_timeout(Duration::from_secs(10)).unwrap();
    let accepted = accept_result.recv_timeout(Duration::from_secs(10)).unwrap();
    match accepted {
        Ok(()) => {
            assert!(live.exists());
            assert_eq!(
                fs::metadata(quarantine.join("accept-ready")).unwrap().len(),
                0
            );
            if fetched.is_ok() {
                assert!(live.join("sessions/conv-bulk.sqlite3").is_file());
            }
        }
        Err(RestoreFault::Tamper) => {
            assert!(
                !live.exists(),
                "a refused accept must leave live data alone"
            );
        }
        Err(error) => panic!("accept stopped with {error:?}"),
    }
    assert!(!live.join("sessions/conv-bulk.sqlite3.partial").exists());
    assert!(!quarantine
        .join("sessions/conv-bulk.sqlite3.partial")
        .exists());
}

#[tokio::test]
async fn accept_holds_the_live_lock_across_promotion() {
    use std::sync::mpsc;
    use std::time::Duration;

    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    let audit = MemoryAudit::open();
    stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    let live = DirectoryQuarantine.anchor(&place.live).unwrap();
    let quarantine = DirectoryQuarantine.anchor(&place.quarantine).unwrap();
    let hold = DirectoryQuarantine.hold(&live).unwrap();
    let (started, ready) = mpsc::channel();
    let (finished, result) = mpsc::channel();
    let accept_live = live.path().to_path_buf();
    std::thread::spawn(move || {
        started.send(()).unwrap();
        let outcome = accept_quarantine(
            &AcceptRequest {
                facts: person(RestoreAction::Accept),
                quarantine: &quarantine,
                live: &accept_live,
                receipts: &NoReceipts,
            },
            &policy(),
            &audit,
            &DirectoryQuarantine,
        );
        finished.send(outcome.map(|_| ())).unwrap();
    });
    ready.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(
        matches!(
            result.recv_timeout(Duration::from_secs(1)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ),
        "accept must hold the live path's lock before promotion"
    );
    assert!(!live.exists());
    drop(hold);
    result
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    assert!(live.exists());
}

struct KeepsTree;

impl QuarantineLayout for KeepsTree {
    fn begin_session(
        &self,
        root: &AnchoredRoot,
        relative: &str,
    ) -> Result<Box<dyn SessionAttempt + '_>, RestoreFault> {
        TestAttempt::begin(TestRetirement::Empty, root, relative)
    }

    fn begin(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
    ) -> Result<Box<dyn Staging>, RestoreFault> {
        Ok(Box::new(FaultyStaging {
            inner: DirectoryQuarantine.begin(quarantine, live)?,
            fault: StageFault {
                ready: ReadyFault::None,
                retain: true,
            },
        }))
    }
    fn load(&self, root: &AnchoredRoot) -> Result<LoadedTree, RestoreFault> {
        DirectoryQuarantine.load(root)
    }
    fn promote(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
        loaded: &LoadedTree,
        generation: &str,
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded, generation)
    }
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }

    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }

    fn exists(&self, path: &Path) -> Result<bool, RestoreFault> {
        DirectoryQuarantine.exists(path)
    }
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.canonicalize(path)
    }
    fn anchor(&self, path: &Path) -> Result<AnchoredRoot, RestoreFault> {
        DirectoryQuarantine.anchor(path)
    }
    fn hold(&self, root: &AnchoredRoot) -> Result<Box<dyn RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }
    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }

    fn session_cut(&self, path: &Path, conversation_id: &str) -> Result<SessionCut, RestoreFault> {
        DirectoryQuarantine.session_cut(path, conversation_id)
    }
}

#[tokio::test]
async fn a_failed_stage_cleanup_still_refuses_acceptance() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    let source = DirectoryVerificationSource::open(bundle.root.clone()).unwrap();
    let error = stage_quarantine(
        &StageRequest {
            facts: person(RestoreAction::Stage),
            bundle: &source,
            format: &ExportFormat,
            verifier: &bundle.key,
            limits: VerificationLimits::default(),
            later_inventory: None,
            quarantine: &place.quarantine,
            live: &place.live,
        },
        &policy(),
        &MemoryAudit::fail_after(1),
        &KeepsTree,
        &DirectoryMaterializer,
        &KeyedInventory::new(bundle.key),
        &OsStagingIds,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Retained);
    assert!(place.quarantine.join("journal.txt").is_file());
    let quarantine = DirectoryQuarantine.anchor(&place.quarantine).unwrap();
    assert!(DirectoryQuarantine.ready(&quarantine).unwrap().is_none());
    let accepted = accept(&place, &MemoryAudit::open(), &NoReceipts).unwrap_err();
    assert_eq!(accepted, RestoreFault::Tamper);
    assert!(!place.live.exists());
}

#[tokio::test]
async fn a_failed_fetch_fallback_empties_the_session() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place, audit) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let source = FakeSource::matching(&session);
    let error = open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &place.live,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &audit.failing_after(2),
        &KeepsTree,
        &source,
        &OsPublicationIds,
        &mut Quiet,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Retained);
    let path = place.live.join("sessions/conv-bulk.sqlite3");
    assert_eq!(fs::metadata(&path).unwrap().len(), 0);
    assert!(!path.with_extension("sqlite3.rejected").exists());
}

/// Remove, rename, and empty all fail once the session file exists.
struct KeepsBytes;

impl QuarantineLayout for KeepsBytes {
    fn begin_session(
        &self,
        root: &AnchoredRoot,
        relative: &str,
    ) -> Result<Box<dyn SessionAttempt + '_>, RestoreFault> {
        TestAttempt::begin(TestRetirement::Retain, root, relative)
    }

    fn begin(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
    ) -> Result<Box<dyn Staging>, RestoreFault> {
        DirectoryQuarantine.begin(quarantine, live)
    }
    fn load(&self, root: &AnchoredRoot) -> Result<LoadedTree, RestoreFault> {
        DirectoryQuarantine.load(root)
    }
    fn promote(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
        loaded: &LoadedTree,
        generation: &str,
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded, generation)
    }
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }

    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }

    fn exists(&self, path: &Path) -> Result<bool, RestoreFault> {
        DirectoryQuarantine.exists(path)
    }
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.canonicalize(path)
    }
    fn anchor(&self, path: &Path) -> Result<AnchoredRoot, RestoreFault> {
        DirectoryQuarantine.anchor(path)
    }
    fn hold(&self, root: &AnchoredRoot) -> Result<Box<dyn RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }
    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }

    fn session_cut(&self, path: &Path, conversation_id: &str) -> Result<SessionCut, RestoreFault> {
        DirectoryQuarantine.session_cut(path, conversation_id)
    }
}

#[tokio::test]
async fn a_failed_fetch_that_cannot_be_retired_is_not_served() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place, audit) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let source = FakeSource::matching(&session);
    let error = open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &place.live,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &audit.failing_after(2),
        &KeepsBytes,
        &source,
        &OsPublicationIds,
        &mut Quiet,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Retained);
    let path = place.live.join("sessions/conv-bulk.sqlite3");
    assert!(fs::metadata(&path).unwrap().len() > 0);
    let again = open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &place.live,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &audit,
        &KeepsBytes,
        &source,
        &OsPublicationIds,
        &mut Quiet,
    )
    .unwrap_err();
    assert_eq!(again, RestoreFault::Retained);
    assert!(fs::metadata(&path).unwrap().len() > 0);
    assert_eq!(*source.writes.lock().unwrap(), 1);
    let anchored = DirectoryQuarantine.anchor(&place.live).unwrap();
    let target = format!("conv-bulk\n{}", anchored.display());
    let publication = DirectoryQuarantine
        .publication(&path)
        .unwrap()
        .expect("the failed availability audit left this publication unaudited");
    assert_ne!(publication, "pub-earlier");
    let earlier = audit.checkpoint();
    earlier
        .append(&RestoreEvidence {
            target: &target,
            before: "held",
            after: "available",
            cause: "fetched",
            initiator: "person",
            decision: "effect",
            policies: &[],
            attestations: "pub-earlier",
            journal_digest: "",
            root_generation: "",
        })
        .unwrap();
    let later = open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &place.live,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &earlier,
        &KeepsBytes,
        &source,
        &OsPublicationIds,
        &mut Quiet,
    )
    .unwrap_err();
    assert_eq!(later, RestoreFault::Retained);
    assert!(fs::metadata(&path).unwrap().len() > 0);
    assert!(!earlier.available("conv-bulk", &publication).unwrap());
    assert!(earlier.available("conv-bulk", "pub-earlier").unwrap());
    assert_eq!(*source.writes.lock().unwrap(), 1);
}

#[tokio::test]
async fn a_promoted_tree_stays_live_when_the_parent_sync_fails() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    let audit = MemoryAudit::open();
    stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    accept_quarantine(
        &AcceptRequest {
            facts: person(RestoreAction::Accept),
            quarantine: &place.quarantine,
            live: &place.live,
            receipts: &NoReceipts,
        },
        &policy(),
        &audit,
        &ReportsUnsynced,
    )
    .unwrap();
    assert!(place.live.join("journal.txt").is_file());
    assert_eq!(
        fs::metadata(place.quarantine.join("accept-ready"))
            .unwrap()
            .len(),
        0
    );
    assert!(audit.lines().iter().any(|line| line.after == "spent"));
    let lines = audit.lines();
    let live = lines.iter().find(|line| line.after == "live").unwrap();
    assert_eq!(live.before, "verified");
    assert_eq!(live.cause, "unsynced");
    assert_eq!(live.decision, "effect");
    let anchored = DirectoryQuarantine.anchor(&place.live).unwrap();
    assert_eq!(live.target, anchored.display().to_string());
    assert!(lines
        .iter()
        .all(|line| !(line.before == "unchanged" && line.after == "unchanged")));
}

/// The rename has happened. The parent sync is reported as not durable.
struct ReportsUnsynced;

impl QuarantineLayout for ReportsUnsynced {
    fn begin_session(
        &self,
        root: &AnchoredRoot,
        relative: &str,
    ) -> Result<Box<dyn SessionAttempt + '_>, RestoreFault> {
        DirectoryQuarantine.begin_session(root, relative)
    }

    fn begin(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
    ) -> Result<Box<dyn Staging>, RestoreFault> {
        DirectoryQuarantine.begin(quarantine, live)
    }
    fn load(&self, root: &AnchoredRoot) -> Result<LoadedTree, RestoreFault> {
        DirectoryQuarantine.load(root)
    }
    fn promote(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
        loaded: &LoadedTree,
        generation: &str,
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded, generation)?;
        Ok(Promoted {
            durable: false,
            source_retirement_failed: false,
            publication_restricted: false,
        })
    }
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }

    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }

    fn exists(&self, path: &Path) -> Result<bool, RestoreFault> {
        DirectoryQuarantine.exists(path)
    }
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.canonicalize(path)
    }
    fn anchor(&self, path: &Path) -> Result<AnchoredRoot, RestoreFault> {
        DirectoryQuarantine.anchor(path)
    }
    fn hold(&self, root: &AnchoredRoot) -> Result<Box<dyn RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }
    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }

    fn session_cut(&self, path: &Path, conversation_id: &str) -> Result<SessionCut, RestoreFault> {
        DirectoryQuarantine.session_cut(path, conversation_id)
    }
}

#[tokio::test]
async fn an_alias_of_the_live_root_reuses_the_fetched_session() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place, audit) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let mut source = FakeSource::matching(&session);
    let first = open_one(&place, "conv-bulk", &source, &audit).unwrap();
    assert!(!first.already_present);
    assert_eq!(first.size, session.size);
    assert_eq!(first.digest, session.digest);
    let path = place.live.join("sessions/conv-bulk.sqlite3");
    let published = fs::read(&path).unwrap();
    assert!(!published.is_empty());
    let sibling = place.live.parent().unwrap().join("other");
    fs::create_dir(&sibling).unwrap();
    let alias = sibling.join("..").join("live");
    let again = open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &alias,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &audit,
        &DirectoryQuarantine,
        &source,
        &OsPublicationIds,
        &mut Quiet,
    )
    .unwrap();
    assert!(again.already_present);
    assert_eq!(*source.writes.lock().unwrap(), 1);
    assert_eq!(fs::read(&path).unwrap(), published);
    assert_eq!(again.size, session.size);
    assert_eq!(again.digest, session.digest);
    source.mode = "missing";
    let third = open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &alias,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &audit,
        &DirectoryQuarantine,
        &source,
        &OsPublicationIds,
        &mut Quiet,
    )
    .unwrap();
    assert!(third.already_present);
    assert_eq!(third.size, session.size);
    assert_eq!(third.digest, session.digest);
    assert_eq!(fs::read(&path).unwrap(), published);
    let cut = DirectoryQuarantine.session_cut(&path, "conv-bulk").unwrap();
    assert_eq!(cut.size, session.size);
    assert_eq!(cut.digest, session.digest);
    assert!(path.is_file());
    assert_eq!(*source.writes.lock().unwrap(), 1);
    let anchored = DirectoryQuarantine.anchor(&place.live).unwrap();
    let target = format!("conv-bulk\n{}", anchored.display());
    assert!(audit.lines().iter().all(|line| line.target == target));
}

#[tokio::test]
async fn a_fetch_in_quarantine_stays_available_after_accept() {
    let session = sealed_session(b"session-v1");
    let bundle = publish(
        note_bundle_rows(),
        deferred_rows("conv-bulk", session.size, session.digest),
        None,
    )
    .await;
    let place = roots();
    let audit = MemoryAudit::open();
    stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    let mut source = FakeSource::matching(&session);
    let fetched = open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &place.quarantine,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &audit,
        &DirectoryQuarantine,
        &source,
        &OsPublicationIds,
        &mut Quiet,
    )
    .unwrap();
    assert!(!fetched.already_present);
    assert_eq!(fetched.size, session.size);
    assert_eq!(fetched.digest, session.digest);
    let published = fs::read(place.quarantine.join("sessions/conv-bulk.sqlite3")).unwrap();
    assert!(!published.is_empty());
    accept(&place, &audit, &NoReceipts).unwrap();
    assert_eq!(
        fs::read(place.live.join("sessions/conv-bulk.sqlite3")).unwrap(),
        published
    );
    source.mode = "missing";
    let again = open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &place.live,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &audit,
        &DirectoryQuarantine,
        &source,
        &OsPublicationIds,
        &mut Quiet,
    )
    .unwrap();
    assert!(again.already_present);
    assert_eq!(*source.writes.lock().unwrap(), 1);
    assert_eq!(
        fs::read(place.live.join("sessions/conv-bulk.sqlite3")).unwrap(),
        published
    );
    assert_eq!(again.size, session.size);
    assert_eq!(again.digest, session.digest);
    let cut = DirectoryQuarantine
        .session_cut(&place.live.join("sessions/conv-bulk.sqlite3"), "conv-bulk")
        .unwrap();
    assert_eq!(cut.size, session.size);
    assert_eq!(cut.digest, session.digest);
}

#[tokio::test]
async fn an_unaudited_session_is_not_promoted() {
    let session = sealed_session(b"session-v1");
    let bundle = publish(
        note_bundle_rows(),
        deferred_rows("conv-bulk", session.size, session.digest),
        None,
    )
    .await;
    let place = roots();
    let audit = MemoryAudit::open();
    stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    let source = FakeSource::matching(&session);
    let error = open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &place.quarantine,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &audit.failing_after(2),
        &KeepsBytes,
        &source,
        &OsPublicationIds,
        &mut Quiet,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Retained);
    assert!(place
        .quarantine
        .join("sessions/conv-bulk.sqlite3")
        .is_file());
    let accepted = accept(&place, &audit, &NoReceipts).unwrap_err();
    assert_eq!(accepted, RestoreFault::Tamper);
    assert!(!place.live.exists());
    assert!(place
        .quarantine
        .join("sessions/conv-bulk.sqlite3")
        .is_file());
}

/// The rename onto the session name succeeded. After that, removing an absent
/// path fails, so retiring the temp name fails. The final name is still retired.
struct ExistsFailsAfterPublish;

impl QuarantineLayout for ExistsFailsAfterPublish {
    fn begin_session(
        &self,
        root: &AnchoredRoot,
        relative: &str,
    ) -> Result<Box<dyn SessionAttempt + '_>, RestoreFault> {
        TestAttempt::begin(TestRetirement::FailPublish, root, relative)
    }

    fn begin(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
    ) -> Result<Box<dyn Staging>, RestoreFault> {
        DirectoryQuarantine.begin(quarantine, live)
    }
    fn load(&self, root: &AnchoredRoot) -> Result<LoadedTree, RestoreFault> {
        DirectoryQuarantine.load(root)
    }
    fn promote(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
        loaded: &LoadedTree,
        generation: &str,
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded, generation)
    }
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }

    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }

    fn exists(&self, path: &Path) -> Result<bool, RestoreFault> {
        DirectoryQuarantine.exists(path)
    }
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.canonicalize(path)
    }
    fn anchor(&self, path: &Path) -> Result<AnchoredRoot, RestoreFault> {
        DirectoryQuarantine.anchor(path)
    }
    fn hold(&self, root: &AnchoredRoot) -> Result<Box<dyn RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }
    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }

    fn session_cut(&self, path: &Path, conversation_id: &str) -> Result<SessionCut, RestoreFault> {
        DirectoryQuarantine.session_cut(path, conversation_id)
    }
}

#[tokio::test]
async fn a_failed_publish_retires_the_final_name_when_the_temp_is_already_gone() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place, audit) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let source = FakeSource::matching(&session);
    let layout = ExistsFailsAfterPublish;
    let error = open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &place.live,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &audit,
        &layout,
        &source,
        &OsPublicationIds,
        &mut Quiet,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Unreadable);
    assert!(!place.live.join("sessions/conv-bulk.sqlite3").exists());
    assert!(partial_names(&place.live).is_empty());
}

struct RefusesStageBegin {
    partial: PathBuf,
    retained: bool,
    begins: AtomicUsize,
}

impl QuarantineLayout for RefusesStageBegin {
    fn begin_session(
        &self,
        root: &AnchoredRoot,
        relative: &str,
    ) -> Result<Box<dyn SessionAttempt + '_>, RestoreFault> {
        DirectoryQuarantine.begin_session(root, relative)
    }
    fn begin(
        &self,
        _quarantine: &AnchoredRoot,
        _live: &AnchoredRoot,
    ) -> Result<Box<dyn Staging>, RestoreFault> {
        self.begins.fetch_add(1, Ordering::SeqCst);
        if self.retained {
            // Model FILE_CREATE succeeding before a constructor's permission
            // readback fails, before a Staging cleanup owner can be returned.
            fs::create_dir(&self.partial).unwrap();
            Err(RestoreFault::Retained)
        } else {
            Err(RestoreFault::Unreadable)
        }
    }
    fn load(&self, root: &AnchoredRoot) -> Result<LoadedTree, RestoreFault> {
        DirectoryQuarantine.load(root)
    }
    fn promote(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
        loaded: &LoadedTree,
        generation: &str,
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded, generation)
    }
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }
    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }
    fn exists(&self, path: &Path) -> Result<bool, RestoreFault> {
        DirectoryQuarantine.exists(path)
    }
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.canonicalize(path)
    }
    fn anchor(&self, path: &Path) -> Result<AnchoredRoot, RestoreFault> {
        DirectoryQuarantine.anchor(path)
    }
    fn hold(&self, root: &AnchoredRoot) -> Result<Box<dyn RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }
    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }
    fn session_cut(&self, path: &Path, conversation_id: &str) -> Result<SessionCut, RestoreFault> {
        DirectoryQuarantine.session_cut(path, conversation_id)
    }
}

async fn assert_stage_begin_refusal(retained: bool, audit_fails: bool) {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    let audit = if audit_fails {
        MemoryAudit::fail_after(1)
    } else {
        MemoryAudit::open()
    };
    let source = DirectoryVerificationSource::open(bundle.root.clone()).unwrap();
    let layout = RefusesStageBegin {
        partial: place.quarantine.with_extension("retained-partial"),
        retained,
        begins: AtomicUsize::new(0),
    };
    let result = stage_quarantine(
        &StageRequest {
            facts: person(RestoreAction::Stage),
            bundle: &source,
            format: &ExportFormat,
            verifier: &bundle.key,
            limits: VerificationLimits::default(),
            later_inventory: None,
            quarantine: &place.quarantine,
            live: &place.live,
        },
        &policy(),
        &audit,
        &layout,
        &DirectoryMaterializer,
        &KeyedInventory::new(bundle.key),
        &OsStagingIds,
    );
    assert_eq!(layout.begins.load(Ordering::SeqCst), 1);
    assert!(!place.quarantine.exists());
    assert!(!place.live.exists());
    assert_eq!(layout.partial.exists(), retained);
    if retained {
        assert_eq!(fs::read_dir(&layout.partial).unwrap().count(), 0);
    }
    let expected = if retained {
        RestoreFault::Retained
    } else if audit_fails {
        RestoreFault::Audit
    } else {
        RestoreFault::Unreadable
    };
    assert_eq!(result.unwrap_err(), expected);
    let lines = audit.lines();
    assert_eq!(
        lines.iter().filter(|line| line.decision == "allow").count(),
        1
    );
    assert!(!lines
        .iter()
        .any(|line| matches!(line.after.as_str(), "available" | "ready" | "verified")));
    if retained {
        assert!(!lines
            .iter()
            .any(|line| line.before == "unchanged" || line.after == "unchanged"));
        if !audit_fails {
            let effect = lines.iter().find(|line| line.after == "unready").unwrap();
            assert_eq!(effect.before, "absent");
            assert_eq!(effect.cause, "retained");
            assert_eq!(effect.decision, "effect");
            assert_eq!(effect.initiator, "person");
            assert_eq!(
                effect.target,
                DirectoryQuarantine
                    .anchor(&place.quarantine)
                    .unwrap()
                    .display()
                    .to_string()
            );
        }
    } else if !audit_fails {
        let effect = lines.iter().find(|line| line.after == "unchanged").unwrap();
        assert_eq!(effect.before, "unchanged");
        assert_eq!(effect.cause, RestoreFault::Unreadable.cause());
        assert!(!lines.iter().any(|line| line.after == "unready"));
    }
}

#[tokio::test]
async fn a_retained_stage_begin_records_the_partial_as_unready() {
    assert_stage_begin_refusal(true, false).await;
}

#[tokio::test]
async fn a_retained_stage_begin_is_not_masked_by_a_failed_audit() {
    assert_stage_begin_refusal(true, true).await;
}

#[tokio::test]
async fn a_stage_begin_refusal_without_effect_preserves_unchanged_audit() {
    assert_stage_begin_refusal(false, false).await;
    assert_stage_begin_refusal(false, true).await;
}

struct RetainedStaging {
    quarantine: PathBuf,
}

impl Staging for RetainedStaging {
    fn write(&mut self, _relative: &str, _bytes: &[u8]) -> Result<(), RestoreFault> {
        Ok(())
    }

    fn finish(self: Box<Self>, _journal: &str) -> Result<Box<dyn StagedTreeOwner>, RestoreFault> {
        fs::create_dir_all(&self.quarantine).unwrap();
        fs::write(self.quarantine.join("journal.txt"), b"kept\n").unwrap();
        Err(RestoreFault::Retained)
    }

    fn discard(self: Box<Self>) -> Result<(), RestoreFault> {
        Ok(())
    }
}

struct RetainsFinish;

impl QuarantineLayout for RetainsFinish {
    fn begin_session(
        &self,
        root: &AnchoredRoot,
        relative: &str,
    ) -> Result<Box<dyn SessionAttempt + '_>, RestoreFault> {
        DirectoryQuarantine.begin_session(root, relative)
    }

    fn begin(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
    ) -> Result<Box<dyn Staging>, RestoreFault> {
        let _ = live;
        Ok(Box::new(RetainedStaging {
            quarantine: quarantine.to_path_buf(),
        }))
    }
    fn load(&self, root: &AnchoredRoot) -> Result<LoadedTree, RestoreFault> {
        DirectoryQuarantine.load(root)
    }
    fn promote(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
        loaded: &LoadedTree,
        generation: &str,
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded, generation)
    }
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }

    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }

    fn exists(&self, path: &Path) -> Result<bool, RestoreFault> {
        DirectoryQuarantine.exists(path)
    }
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.canonicalize(path)
    }
    fn anchor(&self, path: &Path) -> Result<AnchoredRoot, RestoreFault> {
        DirectoryQuarantine.anchor(path)
    }
    fn hold(&self, root: &AnchoredRoot) -> Result<Box<dyn RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }
    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }

    fn session_cut(&self, path: &Path, conversation_id: &str) -> Result<SessionCut, RestoreFault> {
        DirectoryQuarantine.session_cut(path, conversation_id)
    }
}

#[tokio::test]
async fn a_retained_quarantine_is_recorded_unready() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    let audit = MemoryAudit::open();
    let source = DirectoryVerificationSource::open(bundle.root.clone()).unwrap();
    let error = stage_quarantine(
        &StageRequest {
            facts: person(RestoreAction::Stage),
            bundle: &source,
            format: &ExportFormat,
            verifier: &bundle.key,
            limits: VerificationLimits::default(),
            later_inventory: None,
            quarantine: &place.quarantine,
            live: &place.live,
        },
        &policy(),
        &audit,
        &RetainsFinish,
        &DirectoryMaterializer,
        &KeyedInventory::new(bundle.key),
        &OsStagingIds,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Retained);
    assert!(place.quarantine.join("journal.txt").is_file());
    assert!(!place.live.exists());
    let lines = audit.lines();
    let unready = lines.iter().find(|line| line.after == "unready").unwrap();
    assert_eq!(unready.before, "absent");
    assert_eq!(unready.cause, "retained");
    assert!(lines
        .iter()
        .all(|line| !(line.before == "unchanged" && line.after == "unchanged")));
}

#[tokio::test]
async fn an_unreadable_session_body_is_not_sealed_when_a_conversation_is_fenced() {
    let later = DeletionLog::default()
        .append(DeletionFence::conversation("conv-fenced").unwrap())
        .0;
    let bundle = publish(
        vec![Row {
            store: StoreKind::CanonicalRecords,
            identity: "records.sqlite3".into(),
            bytes: Bytes::Public(b"not-a-database".to_vec()),
        }],
        vec![],
        None,
    )
    .await;
    let sealed = seal_inventory(bundle.key, &later);
    let place = roots();
    let error = stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &MemoryAudit::open(),
        person(RestoreAction::Stage),
        Some(&sealed),
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::FencedBody);
    assert!(!place.quarantine.exists());
    assert!(!place.live.exists());
}

struct SwapsDeferredAfterLoad {
    replacement: Vec<u8>,
}

impl QuarantineLayout for SwapsDeferredAfterLoad {
    fn begin_session(
        &self,
        root: &AnchoredRoot,
        relative: &str,
    ) -> Result<Box<dyn SessionAttempt + '_>, RestoreFault> {
        DirectoryQuarantine.begin_session(root, relative)
    }

    fn begin(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
    ) -> Result<Box<dyn Staging>, RestoreFault> {
        DirectoryQuarantine.begin(quarantine, live)
    }

    fn load(&self, root: &AnchoredRoot) -> Result<LoadedTree, RestoreFault> {
        let loaded = DirectoryQuarantine.load(root)?;
        fs::write(root.join("deferred.txt"), &self.replacement)
            .map_err(|_| RestoreFault::Unreadable)?;
        Ok(loaded)
    }

    fn promote(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
        loaded: &LoadedTree,
        generation: &str,
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded, generation)
    }

    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }

    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }

    fn exists(&self, path: &Path) -> Result<bool, RestoreFault> {
        DirectoryQuarantine.exists(path)
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.canonicalize(path)
    }

    fn anchor(&self, path: &Path) -> Result<AnchoredRoot, RestoreFault> {
        DirectoryQuarantine.anchor(path)
    }

    fn hold(&self, root: &AnchoredRoot) -> Result<Box<dyn RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }

    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }

    fn session_cut(&self, path: &Path, conversation_id: &str) -> Result<SessionCut, RestoreFault> {
        DirectoryQuarantine.session_cut(path, conversation_id)
    }
}

#[tokio::test]
async fn a_deferred_line_changed_after_loading_is_not_fetched() {
    let original = sealed_session(b"original");
    let replacement = sealed_session(b"replacement");
    let bundle = publish(
        note_bundle_rows(),
        deferred_rows("conv-bulk", original.size, original.digest),
        None,
    )
    .await;
    let place = roots();
    let audit = MemoryAudit::open();
    stage(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        person(RestoreAction::Stage),
        None,
    )
    .unwrap();
    accept(&place, &audit, &NoReceipts).unwrap();
    let mut lines = crate::backup::domain::decode_deferred(
        &fs::read_to_string(place.live.join("deferred.txt")).unwrap(),
    )
    .unwrap();
    for line in &mut lines {
        line.size = replacement.size;
        line.digest = replacement.digest;
    }
    let layout = SwapsDeferredAfterLoad {
        replacement: crate::backup::domain::encode_deferred(&lines)
            .unwrap()
            .into_bytes(),
    };
    let source = FakeSource::matching(&replacement);
    let result = open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &place.live,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &audit,
        &layout,
        &source,
        &OsPublicationIds,
        &mut Quiet,
    );
    assert_eq!(result.unwrap_err(), RestoreFault::Tamper);
    assert_eq!(*source.writes.lock().unwrap(), 0);
    assert!(!place.live.join("sessions/conv-bulk.sqlite3").exists());
    assert!(!audit.lines().iter().any(|line| line.after == "available"));
}

/// Model the adapter's typed report after the live rename and a failed source retirement.
struct ReportsRetainedSource {
    durable: bool,
    attempts: [AtomicUsize; 2],
}

impl QuarantineLayout for ReportsRetainedSource {
    fn begin_session(
        &self,
        root: &AnchoredRoot,
        relative: &str,
    ) -> Result<Box<dyn SessionAttempt + '_>, RestoreFault> {
        DirectoryQuarantine.begin_session(root, relative)
    }

    fn begin(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
    ) -> Result<Box<dyn Staging>, RestoreFault> {
        DirectoryQuarantine.begin(quarantine, live)
    }
    fn load(&self, root: &AnchoredRoot) -> Result<LoadedTree, RestoreFault> {
        DirectoryQuarantine.load(root)
    }
    fn promote(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
        loaded: &LoadedTree,
        generation: &str,
    ) -> Result<Promoted, RestoreFault> {
        let promoted = crate::backup::infrastructure::promote_with_failed_source_retirement(
            quarantine,
            live,
            loaded,
            generation,
            &self.attempts,
            self.durable,
        )?;
        Ok(Promoted {
            durable: self.durable,
            ..promoted
        })
    }
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }

    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }

    fn exists(&self, path: &Path) -> Result<bool, RestoreFault> {
        DirectoryQuarantine.exists(path)
    }
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.canonicalize(path)
    }
    fn anchor(&self, path: &Path) -> Result<AnchoredRoot, RestoreFault> {
        DirectoryQuarantine.anchor(path)
    }
    fn hold(&self, root: &AnchoredRoot) -> Result<Box<dyn RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }
    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }

    fn session_cut(&self, path: &Path, conversation_id: &str) -> Result<SessionCut, RestoreFault> {
        DirectoryQuarantine.session_cut(path, conversation_id)
    }
}

#[tokio::test]
async fn a_retained_source_does_not_make_the_live_rename_unchanged() {
    for durable in [true, false] {
        let bundle = publish(note_bundle_rows(), vec![], None).await;
        let place = roots();
        let audit = MemoryAudit::open();
        stage(
            &bundle,
            &place.quarantine,
            &place.live,
            &audit,
            person(RestoreAction::Stage),
            None,
        )
        .unwrap();
        let journal_bytes = fs::read(place.quarantine.join("journal.txt")).unwrap();
        let staging_id = fs::read_to_string(place.quarantine.join("accept-ready")).unwrap();
        let digest = Fingerprint::of(&journal_bytes).hex();
        let layout = ReportsRetainedSource {
            durable,
            attempts: [AtomicUsize::new(0), AtomicUsize::new(0)],
        };
        let result = accept_quarantine(
            &AcceptRequest {
                facts: person(RestoreAction::Accept),
                quarantine: &place.quarantine,
                live: &place.live,
                receipts: &NoReceipts,
            },
            &policy(),
            &audit,
            &layout,
        );
        assert_eq!(result.unwrap_err(), RestoreFault::Retained);
        assert_eq!(layout.attempts[0].load(Ordering::SeqCst), 1);
        assert_eq!(layout.attempts[1].load(Ordering::SeqCst), 1);
        assert!(place.live.join("journal.txt").is_file());
        assert_eq!(
            fs::read(place.quarantine.join("journal.txt")).unwrap(),
            journal_bytes
        );
        assert_eq!(
            fs::read_to_string(place.quarantine.join("accept-ready")).unwrap(),
            staging_id
        );
        assert_eq!(
            fs::read_to_string(place.live.join("accept-ready")).unwrap(),
            staging_id
        );
        let source = DirectoryQuarantine.anchor(&place.quarantine).unwrap();
        let live_root = DirectoryQuarantine.anchor(&place.live).unwrap();
        assert_eq!(
            audit
                .sealed(&source.display().to_string(), &digest)
                .unwrap(),
            None
        );
        assert!(audit
            .trusted_journal(
                &live_root.display().to_string(),
                &digest,
                Some(staging_id.trim())
            )
            .unwrap());
        let lines = audit.lines();
        let live = lines.iter().find(|line| line.after == "live").unwrap();
        assert_eq!(live.before, "verified");
        assert_eq!(
            live.cause,
            if durable {
                "retained"
            } else {
                "retained-unsynced"
            }
        );
        assert_eq!(live.decision, "effect");
        assert_eq!(
            live.target,
            DirectoryQuarantine
                .anchor(&place.live)
                .unwrap()
                .display()
                .to_string()
        );
        assert!(lines.iter().any(|line| line.after == "spent"));
        assert!(!lines
            .iter()
            .any(|line| line.before == "unchanged" && line.after == "unchanged"));
    }
}

#[test]
fn a_live_generation_supersedes_an_earlier_staging_at_the_same_path() {
    let audit = MemoryAudit::open();
    let digest_a = Fingerprint::of(b"journal-a").hex();
    let digest_b = Fingerprint::of(b"journal-b").hex();
    let attestation = crate::backup::domain::seal_attestation(&digest_a, "stage-first");
    let sealed = RestoreEvidence {
        target: "/restore",
        before: "absent",
        after: "verified",
        cause: "sealed",
        initiator: "person",
        decision: "effect",
        policies: &[],
        attestations: &attestation,
        journal_digest: "",
        root_generation: "",
    };
    audit.append(&sealed).unwrap();
    audit
        .append(&RestoreEvidence {
            before: "verified",
            after: "ready",
            cause: "ready",
            attestations: "",
            journal_digest: &digest_a,
            root_generation: "stage-first",
            ..sealed
        })
        .unwrap();
    assert_eq!(
        audit.sealed("/restore", &digest_a).unwrap().as_deref(),
        Some("stage-first")
    );
    assert!(audit
        .trusted_journal("/restore", &digest_a, Some("stage-first"))
        .unwrap());
    audit
        .append(&RestoreEvidence {
            before: "verified",
            after: "promoting",
            cause: "accepting",
            attestations: "",
            journal_digest: &digest_b,
            root_generation: "stage-next",
            ..sealed
        })
        .unwrap();
    audit
        .append(&RestoreEvidence {
            before: "verified",
            after: "live",
            cause: "accepted",
            attestations: "",
            journal_digest: &digest_b,
            root_generation: "stage-next",
            ..sealed
        })
        .unwrap();
    assert_eq!(audit.sealed("/restore", &digest_a).unwrap(), None);
    assert!(!audit
        .trusted_journal("/restore", &digest_a, Some("stage-first"))
        .unwrap());
    assert!(audit
        .trusted_journal("/restore", &digest_b, Some("stage-next"))
        .unwrap());
    audit.append(&sealed).unwrap();
    audit
        .append(&RestoreEvidence {
            before: "verified",
            after: "ready",
            cause: "ready",
            attestations: "",
            journal_digest: &digest_a,
            root_generation: "stage-first",
            ..sealed
        })
        .unwrap();
    assert!(!audit
        .trusted_journal("/restore", &digest_b, Some("stage-next"))
        .unwrap());
    assert!(audit
        .trusted_journal("/restore", &digest_a, Some("stage-first"))
        .unwrap());
    audit
        .append(&RestoreEvidence {
            before: "verified",
            after: "unready",
            cause: "unreadable",
            attestations: "stage-first",
            ..sealed
        })
        .unwrap();
    assert_eq!(audit.sealed("/restore", &digest_a).unwrap(), None);
    assert!(!audit
        .trusted_journal("/restore", &digest_a, Some("stage-first"))
        .unwrap());
    assert!(!audit
        .trusted_journal("/restore", &digest_b, Some("stage-next"))
        .unwrap());
}

#[tokio::test]
async fn a_missing_or_unavailable_journal_audit_does_not_fetch() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place, mut audit) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let source = FakeSource::matching(&session);
    assert_eq!(
        open_one(&place, "conv-bulk", &source, &MemoryAudit::open()).unwrap_err(),
        RestoreFault::Tamper,
    );
    audit.trust_unavailable = true;
    assert_eq!(
        open_one(&place, "conv-bulk", &source, &audit).unwrap_err(),
        RestoreFault::Audit,
    );
    assert_eq!(*source.writes.lock().unwrap(), 0);
    assert!(!place.live.join("sessions/conv-bulk.sqlite3").exists());
    audit.trust_unavailable = false;
    assert!(
        !open_one(&place, "conv-bulk", &source, &audit)
            .unwrap()
            .already_present
    );
    assert_eq!(*source.writes.lock().unwrap(), 1);
}

#[tokio::test]
async fn a_failed_stage_without_readiness_does_not_fetch() {
    let session = sealed_session(b"session-v1");
    let bundle = publish(
        note_bundle_rows(),
        deferred_rows("conv-bulk", session.size, session.digest),
        None,
    )
    .await;
    let place = roots();
    let audit = MemoryAudit::fail_after(2);
    let source = DirectoryVerificationSource::open(bundle.root.clone()).unwrap();
    assert_eq!(
        stage_quarantine(
            &StageRequest {
                facts: person(RestoreAction::Stage),
                bundle: &source,
                format: &ExportFormat,
                verifier: &bundle.key,
                limits: VerificationLimits::default(),
                later_inventory: None,
                quarantine: &place.quarantine,
                live: &place.live,
            },
            &policy(),
            &audit,
            &SpendMissed { keep_marker: false },
            &DirectoryMaterializer,
            &KeyedInventory::new(bundle.key),
            &OsStagingIds,
        )
        .unwrap_err(),
        RestoreFault::Retained
    );
    assert!(place.quarantine.join("journal.txt").is_file());
    assert_eq!(
        fs::metadata(place.quarantine.join("accept-ready"))
            .unwrap()
            .len(),
        0
    );
    let source = FakeSource::matching(&session);
    let result = open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &place.quarantine,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &audit.checkpoint(),
        &DirectoryQuarantine,
        &source,
        &OsPublicationIds,
        &mut Quiet,
    );
    assert_eq!(result.unwrap_err(), RestoreFault::Tamper);
    assert_eq!(*source.writes.lock().unwrap(), 0);
    assert!(!place.quarantine.join("sessions/conv-bulk.sqlite3").exists());
}

struct HookAudit<'a, F> {
    inner: &'a MemoryAudit,
    cause: &'static str,
    hook: Mutex<Option<F>>,
    fail: bool,
}

#[cfg(windows)]
fn windows_security_bytes(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "advapi32")]
    extern "system" {
        fn GetFileSecurityW(
            name: *const u16,
            information: u32,
            descriptor: *mut std::ffi::c_void,
            length: u32,
            needed: *mut u32,
        ) -> i32;
    }
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut needed = 0;
    // Owner, group, and DACL include the self-relative descriptor's control
    // flags and each ACE's inheritance flags, not merely rendered permissions.
    let information = 1 | 2 | 4;
    let first = unsafe {
        GetFileSecurityW(
            name.as_ptr(),
            information,
            std::ptr::null_mut(),
            0,
            &mut needed,
        )
    };
    let first_error = std::io::Error::last_os_error();
    assert_eq!(first, 0);
    assert_eq!(first_error.raw_os_error(), Some(122));
    assert!(needed > 0);
    let mut descriptor = vec![0u32; (needed as usize).div_ceil(4)];
    let capacity = (descriptor.len() * 4) as u32;
    // The counted UTF-16 name and aligned writable descriptor storage remain
    // live for the native call; the returned byte count bounds the final slice.
    let read = unsafe {
        GetFileSecurityW(
            name.as_ptr(),
            information,
            descriptor.as_mut_ptr().cast(),
            capacity,
            &mut needed,
        )
    };
    assert_ne!(
        read,
        0,
        "read the actual Windows security descriptor: {}",
        std::io::Error::last_os_error()
    );
    assert!(needed <= capacity);
    unsafe { std::slice::from_raw_parts(descriptor.as_ptr().cast::<u8>(), needed as usize) }
        .to_vec()
}

#[cfg(windows)]
#[tokio::test]
async fn final_available_audit_keeps_owned_session_names_pinned_without_changing_foreign_acls() {
    for fail in [false, true] {
        let session = sealed_session(b"session-v1");
        let (_bundle, place, audit) =
            deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
        let sessions = place.live.join("sessions");
        nessa_local_storage::create_directory(&sessions).unwrap();
        fs::create_dir_all(sessions.join("foreign-directory")).unwrap();
        let foreign = sessions.join("foreign.sqlite3");
        let grandchild = sessions.join("foreign-directory/child");
        fs::write(&foreign, b"foreign session").unwrap();
        fs::write(&grandchild, b"foreign child").unwrap();
        let foreign_paths = [
            sessions.clone(),
            foreign.clone(),
            sessions.join("foreign-directory"),
            grandchild.clone(),
        ];
        let before: Vec<_> = foreign_paths
            .iter()
            .map(|path| windows_security_bytes(path))
            .collect();
        let output = sessions.join("conv-bulk.sqlite3");
        let sidecar = output.with_extension("sqlite3.publication");
        let called = AtomicUsize::new(0);
        let hooked = HookAudit {
            inner: &audit,
            cause: "fetched",
            fail,
            hook: Mutex::new(Some(|| {
                called.fetch_add(1, Ordering::SeqCst);
                assert_eq!(
                    foreign_paths
                        .iter()
                        .map(|path| windows_security_bytes(path))
                        .collect::<Vec<_>>(),
                    before,
                );
                // The final audit follows owned ACL release. ERROR_SHARING_VIOLATION
                // on a new writer proves the retained File's sharing pin still
                // protects each leaf, rather than an unreleased restrictive ACL.
                for path in [&output, &sidecar] {
                    let bytes = fs::read(path).unwrap();
                    let write = fs::OpenOptions::new().write(true).open(path).unwrap_err();
                    assert_eq!(write.raw_os_error(), Some(32));
                    let remove = fs::remove_file(path).unwrap_err();
                    assert!(matches!(remove.raw_os_error(), Some(5 | 32)));
                    let moved = path.with_extension("attacker-moved");
                    let rename = fs::rename(path, &moved).unwrap_err();
                    assert!(matches!(rename.raw_os_error(), Some(5 | 32)));
                    assert!(!moved.exists());
                    assert_eq!(fs::read(path).unwrap(), bytes);
                }
                assert_eq!(fs::read(&sidecar).unwrap(), b"pub-audit-pins\n");
                fs::OpenOptions::new().write(true).open(&foreign).unwrap();
                fs::OpenOptions::new()
                    .write(true)
                    .open(&grandchild)
                    .unwrap();
            })),
        };
        let source = FakeSource::matching(&session);
        let result = open_deferred_conversation(
            &OpenRequest {
                facts: person(RestoreAction::Fetch),
                root: &place.live,
                conversation_id: "conv-bulk",
            },
            &policy(),
            &hooked,
            &DirectoryQuarantine,
            &source,
            &FixedIds("pub-audit-pins"),
            &mut Quiet,
        );
        assert_eq!(called.load(Ordering::SeqCst), 1);
        assert_eq!(*source.writes.lock().unwrap(), 1);
        assert_eq!(
            foreign_paths
                .iter()
                .map(|path| windows_security_bytes(path))
                .collect::<Vec<_>>(),
            before,
        );
        assert_eq!(fs::read(&foreign).unwrap(), b"foreign session");
        assert_eq!(fs::read(&grandchild).unwrap(), b"foreign child");
        if fail {
            assert_eq!(result.unwrap_err(), RestoreFault::Retained);
            assert_eq!(fs::metadata(&output).unwrap().len(), 0);
            assert_eq!(fs::metadata(&sidecar).unwrap().len(), 0);
            assert!(!audit.available("conv-bulk", "pub-audit-pins").unwrap());
            assert!(!audit.lines().iter().any(|line| line.after == "available"));
        } else {
            let report = result.unwrap();
            assert_eq!(report.digest, session.digest);
            assert!(!report.already_present);
            assert!(audit.available("conv-bulk", "pub-audit-pins").unwrap());
            // Normal SQLite pathname opens and mutation resume only after the
            // successful application call drops the publication attempt.
            assert_eq!(
                DirectoryQuarantine
                    .session_cut(&output, "conv-bulk")
                    .unwrap()
                    .digest,
                session.digest
            );
            for path in [&output, &sidecar] {
                let bytes = fs::read(path).unwrap();
                let mut writer = fs::OpenOptions::new().write(true).open(path).unwrap();
                writer.write_all(&bytes).unwrap();
                writer.sync_all().unwrap();
                drop(writer);
                let moved = path.with_extension("consumer-moved");
                fs::rename(path, &moved).unwrap();
                fs::rename(&moved, path).unwrap();
                assert_eq!(fs::read(path).unwrap(), bytes);
            }
        }
    }
}

impl<F: FnOnce() + Send> RestoreAudit for HookAudit<'_, F> {
    fn append(&self, evidence: &RestoreEvidence<'_>) -> Result<(), DecisionAuditError> {
        if evidence.cause == self.cause {
            if let Some(hook) = self.hook.lock().unwrap().take() {
                hook();
                if self.fail {
                    return Err(DecisionAuditError::NotStored);
                }
            }
        }
        self.inner.append(evidence)
    }
    fn available(
        &self,
        conversation_id: &str,
        publication: &str,
    ) -> Result<bool, DecisionAuditError> {
        self.inner.available(conversation_id, publication)
    }
    fn sealed(&self, target: &str, digest: &str) -> Result<Option<String>, DecisionAuditError> {
        self.inner.sealed(target, digest)
    }
    fn trusted_journal(
        &self,
        target: &str,
        digest: &str,
        generation: Option<&str>,
    ) -> Result<bool, DecisionAuditError> {
        self.inner.trusted_journal(target, digest, generation)
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_failed_fetch_audit_retires_the_held_directory_after_an_ancestor_swap() {
    for cause in ["matched", "fetched"] {
        let session = sealed_session(b"session-v1");
        let bundle = publish(
            note_bundle_rows(),
            deferred_rows("conv-bulk", session.size, session.digest),
            None,
        )
        .await;
        let keep = tempfile::tempdir().unwrap();
        let ancestor = keep.path().join("ancestor");
        let saved = keep.path().join("saved");
        let other = keep.path().join("other");
        fs::create_dir(&ancestor).unwrap();
        fs::create_dir_all(other.join("live/sessions")).unwrap();
        let sentinel = other.join("live/sessions/conv-bulk.sqlite3");
        fs::write(&sentinel, b"other-session").unwrap();
        fs::write(
            sentinel.with_extension("sqlite3.publication"),
            b"other-publication\n",
        )
        .unwrap();
        let place = Roots {
            quarantine: ancestor.join("quarantine"),
            live: ancestor.join("live"),
            _keep: keep,
        };
        let audit = MemoryAudit::open();
        stage(
            &bundle,
            &place.quarantine,
            &place.live,
            &audit,
            person(RestoreAction::Stage),
            None,
        )
        .unwrap();
        accept(&place, &audit, &NoReceipts).unwrap();
        let original_ancestor = ancestor.clone();
        let retained_ancestor = saved.clone();
        let replacement = other.clone();
        let hooked = HookAudit {
            inner: &audit,
            cause,
            fail: true,
            hook: Mutex::new(Some(move || {
                fs::rename(&original_ancestor, &retained_ancestor).unwrap();
                std::os::unix::fs::symlink(&replacement, &original_ancestor).unwrap();
            })),
        };
        let source = FakeSource::matching(&session);
        assert_eq!(
            open_one(&place, "conv-bulk", &source, &hooked).unwrap_err(),
            if cause == "matched" {
                RestoreFault::Audit
            } else {
                RestoreFault::Retained
            }
        );
        assert_eq!(fs::read(&sentinel).unwrap(), b"other-session");
        assert_eq!(
            fs::read(sentinel.with_extension("sqlite3.publication")).unwrap(),
            b"other-publication\n"
        );
        if cause == "matched" {
            assert!(!saved.join("live/sessions/conv-bulk.sqlite3").exists());
        } else {
            assert_eq!(
                fs::metadata(saved.join("live/sessions/conv-bulk.sqlite3"))
                    .unwrap()
                    .len(),
                0
            );
        }
        if cause == "matched" {
            assert!(!saved
                .join("live/sessions/conv-bulk.sqlite3.publication")
                .exists());
        } else {
            assert_eq!(
                fs::metadata(saved.join("live/sessions/conv-bulk.sqlite3.publication"))
                    .unwrap()
                    .len(),
                0
            );
        }
        assert!(partial_names(&saved.join("live")).is_empty());
        assert!(!audit.lines().iter().any(|line| line.cause == "fetched"));
    }
}

#[tokio::test]
async fn a_failed_fetch_audit_does_not_retire_a_replacement_sessions_directory() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place, audit) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let sessions = place.live.join("sessions");
    let saved = place.live.join("retained-sessions");
    let swapped = sessions.clone();
    let retained = saved.clone();
    let hooked = HookAudit {
        inner: &audit,
        cause: "fetched",
        fail: true,
        hook: Mutex::new(Some(move || {
            #[cfg(not(windows))]
            {
                fs::rename(&swapped, &retained).unwrap();
                fs::create_dir(&swapped).unwrap();
                fs::write(swapped.join("conv-bulk.sqlite3"), b"replacement").unwrap();
            }
            #[cfg(windows)]
            {
                let error = fs::rename(&swapped, &retained).unwrap_err();
                assert!(
                    matches!(error.raw_os_error(), Some(5) | Some(32)),
                    "the held sessions directory must refuse the actual rename: {error}"
                );
            }
        })),
    };
    assert_eq!(
        open_one(
            &place,
            "conv-bulk",
            &FakeSource::matching(&session),
            &hooked
        )
        .unwrap_err(),
        RestoreFault::Retained
    );
    #[cfg(not(windows))]
    {
        assert_eq!(
            fs::read(sessions.join("conv-bulk.sqlite3")).unwrap(),
            b"replacement"
        );
        assert_eq!(
            fs::metadata(saved.join("conv-bulk.sqlite3")).unwrap().len(),
            0
        );
        assert_eq!(
            fs::metadata(saved.join("conv-bulk.sqlite3.publication"))
                .unwrap()
                .len(),
            0
        );
    }
    #[cfg(windows)]
    {
        assert!(!saved.exists());
        assert_eq!(
            fs::metadata(sessions.join("conv-bulk.sqlite3"))
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            fs::metadata(sessions.join("conv-bulk.sqlite3.publication"))
                .unwrap()
                .len(),
            0
        );
    }
    assert!(!audit.lines().iter().any(|line| line.after == "available"));
}

struct HookIds<F>(Mutex<Option<F>>);
impl<F: FnOnce() + Send> PublicationIds for HookIds<F> {
    fn publication_id(&self) -> Result<String, RestoreFault> {
        self.0.lock().unwrap().take().unwrap()();
        Err(RestoreFault::Unreadable)
    }
}

#[tokio::test]
async fn a_failed_publication_id_does_not_remove_a_replacement_session_inode() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place, audit) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let final_path = place.live.join("sessions/conv-bulk.sqlite3");
    let displaced = final_path.with_extension("sqlite3.displaced");
    let swapped = final_path.clone();
    let retained = displaced.clone();
    let ids = HookIds(Mutex::new(Some(move || {
        #[cfg(not(windows))]
        {
            fs::rename(&swapped, &retained).unwrap();
            fs::write(&swapped, b"replacement-session").unwrap();
        }
        #[cfg(windows)]
        {
            let error = fs::rename(&swapped, &retained).unwrap_err();
            assert!(
                matches!(error.raw_os_error(), Some(5) | Some(32)),
                "the protected session must refuse the actual rename: {error}"
            );
        }
    })));
    let result = open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &place.live,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &audit,
        &DirectoryQuarantine,
        &FakeSource::matching(&session),
        &ids,
        &mut Quiet,
    );
    assert_eq!(result.unwrap_err(), RestoreFault::Retained);
    #[cfg(not(windows))]
    {
        assert_eq!(fs::metadata(&displaced).unwrap().len(), 0);
        assert_eq!(fs::read(&final_path).unwrap(), b"replacement-session");
    }
    #[cfg(windows)]
    {
        assert!(!displaced.exists());
        assert_eq!(fs::metadata(&final_path).unwrap().len(), 0);
    }
    assert!(!audit.lines().iter().any(|line| line.after == "available"));
    assert!(!final_path.with_extension("sqlite3.publication").exists());
}

#[derive(Clone, Copy)]
enum TestRetirement {
    MoveAside,
    Empty,
    Retain,
    RetainPartial,
    FailPublish,
}

/// A named-file test owner for the retained-resource matrix. The modes model
/// adapter outcomes explicitly; real ownership/race tests use DirectoryQuarantine.
struct TestAttempt {
    mode: TestRetirement,
    temp: PathBuf,
    final_path: PathBuf,
    file: fs::File,
    published: bool,
}
impl TestAttempt {
    fn begin(
        mode: TestRetirement,
        root: &AnchoredRoot,
        relative: &str,
    ) -> Result<Box<dyn SessionAttempt>, RestoreFault> {
        let final_path = root.join(relative);
        fs::create_dir_all(final_path.parent().unwrap()).map_err(|_| RestoreFault::Unreadable)?;
        let temp = final_path.with_extension("sqlite3.partial");
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|_| RestoreFault::Unreadable)?;
        Ok(Box::new(Self {
            mode,
            temp,
            final_path,
            file,
            published: false,
        }))
    }
}
impl SessionAttempt for TestAttempt {
    fn destination(&mut self) -> Result<fs::File, RestoreFault> {
        self.file.try_clone().map_err(|_| RestoreFault::Unreadable)
    }
    fn inspect(&self, conversation_id: &str) -> Result<SessionCut, RestoreFault> {
        DirectoryQuarantine.session_cut(&self.temp, conversation_id)
    }
    fn publish(
        &mut self,
        _conversation_id: &str,
        _digest: &Fingerprint,
    ) -> Result<(), RestoreFault> {
        if self.final_path.exists() {
            return Err(RestoreFault::SessionDiffers);
        }
        fs::rename(&self.temp, &self.final_path).map_err(|_| RestoreFault::Rename)?;
        self.published = true;
        if matches!(self.mode, TestRetirement::FailPublish) {
            Err(RestoreFault::Unreadable)
        } else {
            Ok(())
        }
    }
    fn note_publication(&mut self, publication: &str) -> Result<PublishedSession, RestoreFault> {
        fixture_note_publication(&self.final_path, publication);
        Ok(PublishedSession { durable: true })
    }
    fn release(&mut self) -> Result<(), RestoreFault> {
        Ok(())
    }
    fn retire(&mut self) -> Result<(), RestoreFault> {
        if matches!(
            self.mode,
            TestRetirement::Retain | TestRetirement::RetainPartial
        ) {
            return Err(RestoreFault::Retained);
        }
        if self.published {
            match self.mode {
                TestRetirement::MoveAside => fs::rename(
                    &self.final_path,
                    self.final_path.with_extension("sqlite3.rejected"),
                )
                .unwrap(),
                TestRetirement::Empty => fs::write(&self.final_path, b"").unwrap(),
                TestRetirement::FailPublish => fs::remove_file(&self.final_path).unwrap(),
                TestRetirement::Retain | TestRetirement::RetainPartial => unreachable!(),
            }
            if matches!(self.mode, TestRetirement::MoveAside | TestRetirement::Empty) {
                return Err(RestoreFault::Retained);
            }
        } else if self.temp.exists() {
            fs::remove_file(&self.temp).unwrap();
        }
        Ok(())
    }
}
fn fixture_note_publication(session: &Path, publication: &str) {
    fs::write(
        session.with_extension("sqlite3.publication"),
        format!("{publication}\n"),
    )
    .unwrap();
}

struct PublicationOutcomeLayout {
    begin_failure: Option<RestoreFault>,
    keep: bool,
    notes: AtomicUsize,
    release_failure: bool,
    reported_durability: Option<bool>,
}
impl QuarantineLayout for PublicationOutcomeLayout {
    fn begin_session(
        &self,
        root: &AnchoredRoot,
        relative: &str,
    ) -> Result<Box<dyn SessionAttempt + '_>, RestoreFault> {
        if let Some(error) = self.begin_failure {
            return Err(error);
        }
        Ok(Box::new(ControlledPublicationAttempt {
            inner: DirectoryQuarantine.begin_session(root, relative)?,
            keep: self.keep,
            notes: &self.notes,
            release_failure: self.release_failure,
            reported_durability: self.reported_durability,
        }))
    }

    fn begin(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
    ) -> Result<Box<dyn Staging>, RestoreFault> {
        DirectoryQuarantine.begin(quarantine, live)
    }
    fn load(&self, root: &AnchoredRoot) -> Result<LoadedTree, RestoreFault> {
        DirectoryQuarantine.load(root)
    }
    fn promote(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
        loaded: &LoadedTree,
        generation: &str,
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded, generation)
    }

    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }

    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }

    fn exists(&self, path: &Path) -> Result<bool, RestoreFault> {
        DirectoryQuarantine.exists(path)
    }
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.canonicalize(path)
    }
    fn anchor(&self, path: &Path) -> Result<AnchoredRoot, RestoreFault> {
        DirectoryQuarantine.anchor(path)
    }
    fn hold(&self, root: &AnchoredRoot) -> Result<Box<dyn RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }
    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }

    fn session_cut(&self, path: &Path, conversation_id: &str) -> Result<SessionCut, RestoreFault> {
        DirectoryQuarantine.session_cut(path, conversation_id)
    }
}

struct ControlledPublicationAttempt<'a> {
    inner: Box<dyn SessionAttempt + 'a>,
    keep: bool,
    notes: &'a AtomicUsize,
    release_failure: bool,
    reported_durability: Option<bool>,
}
impl SessionAttempt for ControlledPublicationAttempt<'_> {
    fn destination(&mut self) -> Result<fs::File, RestoreFault> {
        self.inner.destination()
    }
    fn inspect(&self, id: &str) -> Result<SessionCut, RestoreFault> {
        self.inner.inspect(id)
    }
    fn publish(&mut self, id: &str, digest: &Fingerprint) -> Result<(), RestoreFault> {
        self.inner.publish(id, digest)
    }
    fn release(&mut self) -> Result<(), RestoreFault> {
        if self.release_failure {
            Err(RestoreFault::Unreadable)
        } else {
            self.inner.release()
        }
    }
    fn note_publication(&mut self, publication: &str) -> Result<PublishedSession, RestoreFault> {
        self.notes.fetch_add(1, Ordering::SeqCst);
        self.inner
            .note_publication(publication)
            .map(|published| PublishedSession {
                durable: self.reported_durability.unwrap_or(published.durable),
            })
    }
    fn retire(&mut self) -> Result<(), RestoreFault> {
        if self.keep {
            Err(RestoreFault::Retained)
        } else {
            self.inner.retire()
        }
    }
}

#[tokio::test]
async fn retained_fetch_setup_does_not_claim_unchanged_or_start_the_source() {
    for unavailable_audit in [false, true] {
        let session = sealed_session(b"session-v1");
        let (_bundle, place, audit) =
            deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
        let audit = if unavailable_audit {
            audit.failing_after(1)
        } else {
            audit.checkpoint()
        };
        let layout = PublicationOutcomeLayout {
            begin_failure: Some(RestoreFault::Retained),
            keep: false,
            notes: AtomicUsize::new(0),
            release_failure: false,
            reported_durability: None,
        };
        let source = FakeSource::matching(&session);
        let result = open_deferred_conversation(
            &OpenRequest {
                facts: person(RestoreAction::Fetch),
                root: &place.live,
                conversation_id: "conv-bulk",
            },
            &policy(),
            &audit,
            &layout,
            &source,
            &OsPublicationIds,
            &mut Quiet,
        );
        assert_eq!(result.unwrap_err(), RestoreFault::Retained);
        assert_eq!(*source.writes.lock().unwrap(), 0);
        assert_eq!(layout.notes.load(Ordering::SeqCst), 0);
        assert!(!place.live.join("sessions/conv-bulk.sqlite3").exists());
        let lines = audit.lines();
        assert!(lines.iter().any(|line| line.decision == "allow"));
        assert!(!lines.iter().any(|line| line.after == "available"));
        assert!(!lines.iter().any(|line| line.before == "unchanged"));
    }
}

#[tokio::test]
async fn a_failed_release_never_records_or_reuses_an_available_receipt() {
    for (keep, unavailable_audit) in [(true, false), (false, false), (true, true), (false, true)] {
        let session = sealed_session(b"session-v1");
        let (_bundle, place, audit) =
            deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
        // Cross a working sink with a sink that fails after decision+match.
        let audit = if unavailable_audit {
            audit.failing_after(2)
        } else {
            audit.checkpoint()
        };
        let layout = PublicationOutcomeLayout {
            begin_failure: None,
            keep,
            notes: AtomicUsize::new(0),
            release_failure: true,
            reported_durability: None,
        };
        let source = FakeSource::matching(&session);
        let result = open_deferred_conversation(
            &OpenRequest {
                facts: person(RestoreAction::Fetch),
                root: &place.live,
                conversation_id: "conv-bulk",
            },
            &policy(),
            &audit,
            &layout,
            &source,
            &FixedIds("pub-release-failed"),
            &mut Quiet,
        );
        assert_eq!(result.unwrap_err(), RestoreFault::Retained);
        assert_eq!(layout.notes.load(Ordering::SeqCst), 1);
        assert!(!audit.available("conv-bulk", "pub-release-failed").unwrap());
        let session_path = place.live.join("sessions/conv-bulk.sqlite3");
        if keep {
            assert_eq!(
                DirectoryQuarantine
                    .publication(&session_path)
                    .unwrap()
                    .as_deref(),
                Some("pub-release-failed")
            );
        } else {
            assert_eq!(
                fs::metadata(session_path.with_extension("sqlite3.publication"))
                    .unwrap()
                    .len(),
                0
            );
        }
        if keep {
            assert!(fs::metadata(&session_path).unwrap().len() > 0);
        } else {
            assert_eq!(fs::metadata(&session_path).unwrap().len(), 0);
        }
        assert_eq!(
            open_one(&place, "conv-bulk", &source, &audit.checkpoint()).unwrap_err(),
            if keep {
                RestoreFault::Retained
            } else {
                RestoreFault::Tamper
            }
        );
        assert_eq!(*source.writes.lock().unwrap(), 1);
    }
}

#[tokio::test]
async fn a_reused_publication_id_does_not_borrow_an_earlier_receipt() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place, audit) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    audit
        .append(&RestoreEvidence {
            target: "conv-bulk\n/earlier-root",
            before: "held",
            after: "available",
            cause: "fetched",
            initiator: "person",
            decision: "effect",
            policies: &[],
            attestations: "pub-reused",
            journal_digest: "",
            root_generation: "",
        })
        .unwrap();
    let source = FakeSource::matching(&session);
    let result = open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &place.live,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &audit,
        &DirectoryQuarantine,
        &source,
        &FixedIds("pub-reused"),
        &mut Quiet,
    );
    assert_eq!(result.unwrap_err(), RestoreFault::Retained);
    let path = place.live.join("sessions/conv-bulk.sqlite3");
    assert_eq!(fs::metadata(&path).unwrap().len(), 0);
    assert!(DirectoryQuarantine.publication(&path).unwrap().is_none());
    assert_eq!(
        audit
            .lines()
            .iter()
            .filter(|line| line.cause == "fetched")
            .count(),
        1
    );
    assert_eq!(*source.writes.lock().unwrap(), 1);
}

#[tokio::test]
async fn a_committed_session_reports_its_sync_result_and_cached_durability_is_unknown() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place, audit) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let layout = PublicationOutcomeLayout {
        begin_failure: None,
        keep: false,
        notes: AtomicUsize::new(0),
        release_failure: false,
        reported_durability: Some(false),
    };
    let source = FakeSource::matching(&session);
    let report = open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &place.live,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &audit,
        &layout,
        &source,
        &OsPublicationIds,
        &mut Quiet,
    )
    .unwrap();
    assert_eq!(report.durable, Some(false));
    assert!(!report.already_present);
    let cached = open_one(&place, "conv-bulk", &source, &audit).unwrap();
    assert!(cached.already_present);
    assert_eq!(cached.durable, None);
    assert_eq!(*source.writes.lock().unwrap(), 1);
}

fn snapshot_tree_files(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn descend(root: &Path, directory: &Path, result: &mut Vec<(PathBuf, Vec<u8>)>) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                descend(root, &entry.path(), result);
            } else {
                result.push((
                    entry.path().strip_prefix(root).unwrap().to_owned(),
                    fs::read(entry.path()).unwrap(),
                ));
            }
        }
    }
    let mut result = Vec::new();
    descend(root, root, &mut result);
    result.sort_by(|left, right| left.0.cmp(&right.0));
    result
}

#[tokio::test]
async fn restoring_every_old_live_byte_after_a_failed_new_acceptance_does_not_restore_authority() {
    for same_digest in [true, false] {
        let original = sealed_session(b"old-version");
        let newer = sealed_session(b"new-version");
        let bundle_a = publish(
            note_bundle_rows(),
            deferred_rows("conv-bulk", original.size, original.digest),
            None,
        )
        .await;
        let selected = if same_digest { &original } else { &newer };
        let bundle_b = publish(
            note_bundle_rows(),
            deferred_rows("conv-bulk", selected.size, selected.digest),
            None,
        )
        .await;
        let mut place = roots();
        let audit = MemoryAudit::open();
        stage(
            &bundle_a,
            &place.quarantine,
            &place.live,
            &audit,
            person(RestoreAction::Stage),
            None,
        )
        .unwrap();
        accept(&place, &audit, &NoReceipts).unwrap();
        let old_bytes = snapshot_tree_files(&place.live);
        let old_generation = fs::read(place.live.join("accept-ready")).unwrap();
        let parked_old = place._keep.path().join("parked-old");
        fs::rename(&place.live, &parked_old).unwrap();
        place.quarantine = place._keep.path().join("quarantine-b");
        stage(
            &bundle_b,
            &place.quarantine,
            &place.live,
            &audit,
            person(RestoreAction::Stage),
            None,
        )
        .unwrap();
        let new_generation = fs::read(place.quarantine.join("accept-ready")).unwrap();
        assert_ne!(new_generation, old_generation);
        let failed_audit = FailsAfterPromotionIntent {
            inner: audit.checkpoint(),
            target: DirectoryQuarantine
                .anchor(&place.live)
                .unwrap()
                .display()
                .to_string(),
            failed: Mutex::new(false),
        };
        assert_eq!(
            accept(&place, &failed_audit, &NoReceipts).unwrap_err(),
            RestoreFault::Audit
        );
        assert!(
            DirectoryQuarantine
                .ready(&DirectoryQuarantine.anchor(&place.quarantine).unwrap())
                .unwrap()
                .is_none(),
            "the source was confirmed unready after the actual live rename"
        );
        assert_eq!(
            fs::read(place.live.join("accept-ready")).unwrap(),
            new_generation
        );
        let parked_new = place._keep.path().join("parked-new");
        fs::rename(&place.live, &parked_new).unwrap();
        fs::rename(&parked_old, &place.live).unwrap();
        assert_eq!(
            snapshot_tree_files(&place.live),
            old_bytes,
            "the full original image and marker were restored"
        );
        let source = FakeSource::matching(&original);
        let resumed_audit = failed_audit.inner.checkpoint();
        assert_eq!(
            open_one(&place, "conv-bulk", &source, &resumed_audit).unwrap_err(),
            RestoreFault::Tamper
        );
        assert_eq!(*source.writes.lock().unwrap(), 0);
        assert!(!place.live.join("sessions/conv-bulk.sqlite3").exists());
        // A fresh, completely acknowledged generation remains a valid recovery.
        fs::rename(&place.live, &parked_old).unwrap();
        place.quarantine = place._keep.path().join("quarantine-c");
        stage(
            &bundle_a,
            &place.quarantine,
            &place.live,
            &resumed_audit,
            person(RestoreAction::Stage),
            None,
        )
        .unwrap();
        accept(&place, &resumed_audit, &NoReceipts).unwrap();
        assert!(
            !open_one(&place, "conv-bulk", &source, &resumed_audit)
                .unwrap()
                .already_present
        );
        assert_eq!(*source.writes.lock().unwrap(), 1);
    }
}

#[derive(Clone, Copy)]
enum ReadyFault {
    None,
    Before,
    After,
}
#[derive(Clone, Copy)]
struct StageFault {
    ready: ReadyFault,
    retain: bool,
}
struct FaultyStaging {
    inner: Box<dyn Staging>,
    fault: StageFault,
}
impl Staging for FaultyStaging {
    fn write(&mut self, relative: &str, bytes: &[u8]) -> Result<(), RestoreFault> {
        self.inner.write(relative, bytes)
    }
    fn finish(self: Box<Self>, journal: &str) -> Result<Box<dyn StagedTreeOwner>, RestoreFault> {
        Ok(Box::new(FaultyStageOwner {
            inner: self.inner.finish(journal)?,
            fault: self.fault,
        }))
    }
    fn discard(self: Box<Self>) -> Result<(), RestoreFault> {
        self.inner.discard()
    }
}
struct FaultyStageOwner {
    inner: Box<dyn StagedTreeOwner>,
    fault: StageFault,
}
impl StagedTreeOwner for FaultyStageOwner {
    fn check_binding(&self) -> Result<(), RestoreFault> {
        self.inner.check_binding()
    }
    fn mark_ready(&mut self, id: &str) -> Result<(), RestoreFault> {
        match self.fault.ready {
            ReadyFault::None => self.inner.mark_ready(id),
            ReadyFault::Before => Err(RestoreFault::Unreadable),
            ReadyFault::After => {
                self.inner.mark_ready(id)?;
                Err(RestoreFault::Unreadable)
            }
        }
    }
    fn retire(&mut self) -> Result<(), RestoreFault> {
        if self.fault.retain {
            Err(RestoreFault::Retained)
        } else {
            self.inner.retire()
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_failed_stage_audit_retires_only_its_held_tree_after_namespace_substitution() {
    for swap_ancestor in [false, true] {
        let bundle = publish(note_bundle_rows(), vec![], None).await;
        let keep = tempfile::tempdir().unwrap();
        let ancestor = keep.path().join("parent");
        fs::create_dir(&ancestor).unwrap();
        let quarantine = ancestor.join("quarantine");
        let live = keep.path().join("live");
        let saved = keep.path().join("saved");
        let substitute = keep.path().join("substitute");
        fs::create_dir(&substitute).unwrap();
        let replacement = if swap_ancestor {
            substitute.join("quarantine")
        } else {
            quarantine.clone()
        };
        let audit = MemoryAudit::open();
        let hook_quarantine = quarantine.clone();
        let hook_ancestor = ancestor.clone();
        let hook_saved = saved.clone();
        let hook_substitute = substitute.clone();
        let hooked = HookAudit {
            inner: &audit,
            cause: "sealed",
            fail: true,
            hook: Mutex::new(Some(move || {
                if swap_ancestor {
                    fs::rename(&hook_ancestor, &hook_saved).unwrap();
                    fs::create_dir(hook_substitute.join("quarantine")).unwrap();
                    fs::write(
                        hook_substitute.join("quarantine/journal.txt"),
                        b"other-journal",
                    )
                    .unwrap();
                    std::os::unix::fs::symlink(&hook_substitute, &hook_ancestor).unwrap();
                } else {
                    fs::rename(&hook_quarantine, &hook_saved).unwrap();
                    fs::create_dir(&hook_quarantine).unwrap();
                    fs::write(hook_quarantine.join("journal.txt"), b"other-journal").unwrap();
                }
            })),
        };
        assert_eq!(
            stage(
                &bundle,
                &quarantine,
                &live,
                &hooked,
                person(RestoreAction::Stage),
                None
            )
            .unwrap_err(),
            RestoreFault::Retained
        );
        assert_eq!(
            fs::read(replacement.join("journal.txt")).unwrap(),
            b"other-journal"
        );
        let owned = if swap_ancestor {
            saved.join("quarantine")
        } else {
            saved
        };
        let retired = snapshot_tree_files(&owned);
        assert!(!retired.is_empty());
        assert!(
            retired.iter().all(|(_, bytes)| bytes.is_empty()),
            "all exact owned file handles were retired"
        );
        assert!(!owned.join("accept-ready").exists());
        assert!(!live.exists());
        assert!(!audit.lines().iter().any(|line| line.cause == "ready"));
    }
}

#[tokio::test]
async fn a_failed_postpromotion_audit_cannot_publish_a_retained_generation_twice() {
    let bundle = publish(note_bundle_rows(), vec![], None).await;
    let place = roots();
    let audit = MemoryAudit::open();
    stage_with_id(
        &bundle,
        &place.quarantine,
        &place.live,
        &audit,
        &FixedStage("stage-once"),
    )
    .unwrap();
    let journal = fs::read(place.quarantine.join("journal.txt")).unwrap();
    let digest = Fingerprint::of(&journal).hex();
    let label = DirectoryQuarantine
        .anchor(&place.quarantine)
        .unwrap()
        .display()
        .to_string();
    assert_eq!(
        audit.sealed(&label, &digest).unwrap().as_deref(),
        Some("stage-once")
    );
    let marker = fs::read(place.quarantine.join("accept-ready")).unwrap();
    // Persist the live intent, then fail every following append. This targets
    // the old post-rename source spend and the repaired live completion alike.
    let failed_audit = FailsAfterPromotionIntent {
        inner: audit.checkpoint(),
        target: DirectoryQuarantine
            .anchor(&place.live)
            .unwrap()
            .display()
            .to_string(),
        failed: Mutex::new(false),
    };
    let layout = ReportsRetainedSource {
        durable: true,
        attempts: [AtomicUsize::new(0), AtomicUsize::new(0)],
    };
    assert_eq!(
        accept_quarantine(
            &AcceptRequest {
                facts: person(RestoreAction::Accept),
                quarantine: &place.quarantine,
                live: &place.live,
                receipts: &NoReceipts,
            },
            &policy(),
            &failed_audit,
            &layout
        )
        .unwrap_err(),
        RestoreFault::Audit
    );
    assert_eq!(layout.attempts[0].load(Ordering::SeqCst), 1);
    assert_eq!(layout.attempts[1].load(Ordering::SeqCst), 1);
    assert_eq!(fs::read(place.live.join("journal.txt")).unwrap(), journal);
    assert_eq!(
        fs::read(place.quarantine.join("journal.txt")).unwrap(),
        journal
    );
    assert_eq!(
        fs::read(place.quarantine.join("accept-ready")).unwrap(),
        marker
    );
    assert_eq!(
        failed_audit.inner.lines().last().unwrap().after,
        "promoting"
    );
    assert!(!failed_audit
        .inner
        .lines()
        .iter()
        .any(|line| line.after == "live"));
    let first_image = snapshot_tree_files(&place.live);
    let resumed = failed_audit.inner.checkpoint();
    let another_live = place._keep.path().join("another-live");
    // A fresh target and a recovered audit sink cannot reuse this generation.
    assert_eq!(
        accept_quarantine(
            &AcceptRequest {
                facts: person(RestoreAction::Accept),
                quarantine: &place.quarantine,
                live: &another_live,
                receipts: &NoReceipts,
            },
            &policy(),
            &resumed,
            &DirectoryQuarantine
        )
        .unwrap_err(),
        RestoreFault::Tamper
    );
    assert!(!another_live.exists());
    assert_eq!(snapshot_tree_files(&place.live), first_image);
    assert_eq!(resumed.sealed(&label, &digest).unwrap(), None);
    let fresh = roots();
    stage_with_id(
        &bundle,
        &fresh.quarantine,
        &fresh.live,
        &resumed,
        &FixedStage("stage-fresh"),
    )
    .unwrap();
    accept(&fresh, &resumed, &NoReceipts).unwrap();
    assert_eq!(fs::read(fresh.live.join("journal.txt")).unwrap(), journal);
}

/// Fail writes only after the actual live-target promotion intent was stored.
struct FailsAfterPromotionIntent {
    inner: MemoryAudit,
    target: String,
    failed: Mutex<bool>,
}
impl RestoreAudit for FailsAfterPromotionIntent {
    fn append(&self, evidence: &RestoreEvidence<'_>) -> Result<(), DecisionAuditError> {
        let mut failed = self
            .failed
            .lock()
            .map_err(|_| DecisionAuditError::NotStored)?;
        if *failed {
            return Err(DecisionAuditError::NotStored);
        }
        self.inner.append(evidence)?;
        if evidence.target == self.target
            && evidence.before == "verified"
            && evidence.after == "promoting"
            && evidence.cause == "accepting"
            && evidence.decision == "effect"
        {
            *failed = true;
        }
        Ok(())
    }
    fn available(
        &self,
        conversation_id: &str,
        publication: &str,
    ) -> Result<bool, DecisionAuditError> {
        self.inner.available(conversation_id, publication)
    }
    fn sealed(&self, target: &str, digest: &str) -> Result<Option<String>, DecisionAuditError> {
        self.inner.sealed(target, digest)
    }
    fn trusted_journal(
        &self,
        target: &str,
        digest: &str,
        generation: Option<&str>,
    ) -> Result<bool, DecisionAuditError> {
        self.inner.trusted_journal(target, digest, generation)
    }
}

#[tokio::test]
async fn acceptance_audit_failures_before_promotion_preserve_the_consumption_boundary() {
    for fail_live_intent in [false, true] {
        let bundle = publish(note_bundle_rows(), vec![], None).await;
        let place = roots();
        let audit = MemoryAudit::open();
        stage_with_id(
            &bundle,
            &place.quarantine,
            &place.live,
            &audit,
            &FixedStage("stage-before-effects"),
        )
        .unwrap();
        let before = snapshot_tree_files(&place.quarantine);
        let journal = fs::read(place.quarantine.join("journal.txt")).unwrap();
        let digest = Fingerprint::of(&journal).hex();
        let label = DirectoryQuarantine
            .anchor(&place.quarantine)
            .unwrap()
            .display()
            .to_string();
        let hooked = HookAudit {
            inner: &audit,
            cause: if fail_live_intent {
                "accepting"
            } else {
                "spent"
            },
            fail: true,
            hook: Mutex::new(Some(|| {})),
        };
        let layout = ReportsRetainedSource {
            durable: true,
            attempts: [AtomicUsize::new(0), AtomicUsize::new(0)],
        };
        assert_eq!(
            accept_quarantine(
                &AcceptRequest {
                    facts: person(RestoreAction::Accept),
                    quarantine: &place.quarantine,
                    live: &place.live,
                    receipts: &NoReceipts
                },
                &policy(),
                &hooked,
                &layout
            )
            .unwrap_err(),
            RestoreFault::Audit
        );
        assert_eq!(layout.attempts[0].load(Ordering::SeqCst), 0);
        assert_eq!(layout.attempts[1].load(Ordering::SeqCst), 0);
        assert_eq!(snapshot_tree_files(&place.quarantine), before);
        assert!(!place.live.exists());
        assert_eq!(
            audit
                .lines()
                .iter()
                .filter(|line| line.after == "spent")
                .count(),
            usize::from(fail_live_intent)
        );
        assert!(!audit
            .lines()
            .iter()
            .any(|line| line.after == "promoting" || line.after == "live"));
        let resumed = audit.checkpoint();
        if fail_live_intent {
            assert_eq!(resumed.sealed(&label, &digest).unwrap(), None);
            assert_eq!(
                accept(&place, &resumed, &NoReceipts).unwrap_err(),
                RestoreFault::Tamper
            );
            assert!(!place.live.exists());
        } else {
            assert_eq!(
                resumed.sealed(&label, &digest).unwrap().as_deref(),
                Some("stage-before-effects")
            );
            accept(&place, &resumed, &NoReceipts).unwrap();
            assert_eq!(fs::read(place.live.join("journal.txt")).unwrap(), journal);
        }
        let fresh = roots();
        stage_with_id(
            &bundle,
            &fresh.quarantine,
            &fresh.live,
            &resumed,
            &FixedStage("stage-recovered"),
        )
        .unwrap();
        accept(&fresh, &resumed, &NoReceipts).unwrap();
        assert_eq!(fs::read(fresh.live.join("journal.txt")).unwrap(), journal);
    }
}

#[cfg(windows)]
#[tokio::test]
async fn real_windows_publications_report_unconfirmed_directory_durability() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place, audit) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let live = audit
        .history
        .iter()
        .find(|line| line.after == "live")
        .unwrap();
    assert_eq!(live.cause, "unsynced");
    let source = FakeSource::matching(&session);
    let report = open_one(&place, "conv-bulk", &source, &audit).unwrap();
    assert_eq!(report.durable, Some(false));
    assert!(!report.already_present);
    assert_eq!(
        open_one(&place, "conv-bulk", &source, &audit)
            .unwrap()
            .durable,
        None
    );
    assert_eq!(*source.writes.lock().unwrap(), 1);
}

#[tokio::test]
async fn a_deferred_source_cannot_publish_an_additional_conversation() {
    for extra_stream in [true, false] {
        let session = sealed_session(b"authorized-cut");
        let (_bundle, place, audit) =
            deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
        let mut source = FakeSource::matching(&session);
        if extra_stream {
            source.bytes = session_database(&[
                ("conv-bulk", b"authorized-cut"),
                ("conv-fenced", b"unrequested-private-record"),
            ]);
        } else {
            let keep = tempfile::tempdir().unwrap();
            let path = keep.path().join("source.sqlite3");
            fs::write(&path, &source.bytes).unwrap();
            let database = nessa_local_database::rusqlite::Connection::open(&path).unwrap();
            database.execute("INSERT INTO event_records(stream_key,offset,event_id,schema_id,schema_version,payload) SELECT 999,offset,'unrequested-event',schema_id,schema_version,?1 FROM event_records LIMIT 1",[b"orphan-private-record".as_slice()]).unwrap();
            drop(database);
            source.bytes = fs::read(path).unwrap();
        }
        let result = open_one(&place, "conv-bulk", &source, &audit);
        assert_eq!(result.unwrap_err(), RestoreFault::Tamper);
        assert_eq!(*source.writes.lock().unwrap(), 1);
        assert!(!place.live.join("sessions/conv-bulk.sqlite3").exists());
        assert!(!place
            .live
            .join("sessions/conv-bulk.sqlite3.publication")
            .exists());
        assert!(!audit.lines().iter().any(|line| line.after == "available"));
        let matching = FakeSource::matching(&session);
        assert!(
            !open_one(&place, "conv-bulk", &matching, &audit)
                .unwrap()
                .already_present
        );
        assert_eq!(*matching.writes.lock().unwrap(), 1);
        let path = place.live.join("sessions/conv-bulk.sqlite3");
        let database = nessa_local_database::rusqlite::Connection::open(&path).unwrap();
        assert_eq!(
            database
                .query_row("SELECT count(*) FROM event_streams", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            database
                .query_row("SELECT count(*) FROM event_stream_names", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            database
                .query_row("SELECT public_id FROM event_stream_names", [], |row| row
                    .get::<_, String>(
                    0
                ))
                .unwrap(),
            "conv-bulk"
        );
    }
}

#[tokio::test]
async fn a_deferred_source_side_data_is_excluded_from_the_published_image() {
    let session = sealed_session(b"authorized-cut");
    let (_bundle, place, audit) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let mut source = FakeSource::matching(&session);
    let keep = tempfile::tempdir().unwrap();
    let path = keep.path().join("source.sqlite3");
    fs::write(&path, &source.bytes).unwrap();
    let marker = b"PR1066-unrequested-private-side-data-7e40f56bd9c145d5";
    let secret = marker.repeat(128);
    let database = nessa_local_database::rusqlite::Connection::open(&path).unwrap();
    database.execute_batch("CREATE TABLE private_side_data(payload BLOB); ALTER TABLE event_records ADD COLUMN unrelated BLOB;").unwrap();
    database
        .execute(
            "INSERT INTO private_side_data VALUES (?1)",
            [secret.as_slice()],
        )
        .unwrap();
    database
        .execute("UPDATE event_records SET unrelated=?1", [secret.as_slice()])
        .unwrap();
    drop(database);
    source.bytes = fs::read(path).unwrap();
    assert!(source
        .bytes
        .windows(marker.len())
        .any(|bytes| bytes == marker));
    let report = open_one(&place, "conv-bulk", &source, &audit).unwrap();
    assert_eq!(report.size, session.size);
    assert_eq!(report.digest, session.digest);
    assert!(!report.already_present);
    assert_eq!(*source.writes.lock().unwrap(), 1);
    let published = place.live.join("sessions/conv-bulk.sqlite3");
    let bytes = fs::read(&published).unwrap();
    assert!(!bytes.windows(marker.len()).any(|bytes| bytes == marker));
    let database = nessa_local_database::rusqlite::Connection::open(published).unwrap();
    assert_eq!(
        database
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name='private_side_data'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        database
            .query_row(
                "SELECT count(*) FROM pragma_table_info('event_records') WHERE name='unrelated'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        database
            .query_row("SELECT count(*) FROM event_streams", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(
        open_one(&place, "conv-bulk", &source, &audit)
            .unwrap()
            .already_present
    );
    assert_eq!(*source.writes.lock().unwrap(), 1);
}
