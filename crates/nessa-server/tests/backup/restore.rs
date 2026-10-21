//! Quarantine, acceptance, and on-demand fetch of a deferred conversation.
//!
//! A failed or partial stage leaves the live path untouched. Acceptance is a
//! rename onto an empty live path. An oversized conversation is fetched only
//! when it is opened, and only when its stream still matches the cut.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;

use crate::backup::application::Authenticator;
use crate::backup::application::{
    accept_quarantine, current_staging_id, open_deferred_conversation, ordered_roots, root_posture,
    stage_quarantine, AcceptRequest, AnchoredRoot, DecisionAuditError, DeferredSessionSource,
    ExportClock, FetchProgress, InventoryCause, InventoryDecision, LeaseReceipts, LoadedTree,
    MacKey, ManifestPublisher, MutationAdmission, OpenRequest, Promoted, PublicationIds,
    QuarantineLayout, RestoreAction, RestoreAudit, RestoreEvidence, RestoreFacts,
    RestorePolicyEvaluator, SealFields, SessionCut, SessionSourceError, StageRequest, Staging,
    StagingIds, TreeId, VerificationFailure, VerificationLimits, VerificationSource,
};
use crate::backup::domain::{
    encode_journal, read_journal, seal_staging_id, AuthenticatedManifest, CapturedRecord,
    CredentialKind, DeferredEntry, DeletionFence, DeletionLog, Fingerprint, GrantRole, Manifest,
    Posture, RestoreFault, Sensitivity, SourceCut, StoreKind, Watermark, WRITER_INVENTORY,
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
}

struct MemoryAudit {
    fail_at: Option<usize>,
    lines: Mutex<Vec<AuditLine>>,
}

impl MemoryAudit {
    fn open() -> Self {
        Self {
            fail_at: None,
            lines: Mutex::new(Vec::new()),
        }
    }

    fn fail_after(count: usize) -> Self {
        Self {
            fail_at: Some(count),
            lines: Mutex::new(Vec::new()),
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
        Ok(lines.iter().any(|line| {
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
            lines.iter().map(|line| SealFields {
                target: &line.target,
                before: &line.before,
                after: &line.after,
                cause: &line.cause,
                decision: &line.decision,
                attestations: &line.attestations,
            }),
            target,
            digest,
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
    async fn hold(
        &self,
    ) -> Result<
        Box<dyn crate::backup::application::AdmissionLease + Send + Sync>,
        crate::backup::application::AdmissionFailure,
    > {
        Ok(Box::new(OpenHold))
    }
}

struct OpenHold;

impl crate::backup::application::AdmissionLease for OpenHold {
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
                published.push(crate::backup::application::PublishedRecord {
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
        published.push(crate::backup::application::PublishedRecord {
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
) -> Result<crate::backup::application::StageReport, RestoreFault> {
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
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded)
    }

    fn remove_tree(&self, _path: &Path) -> Result<(), RestoreFault> {
        Err(RestoreFault::Unreadable)
    }

    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }

    fn session_temp(&self, root: &AnchoredRoot, relative: &str) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.session_temp(root, relative)
    }

    fn publish_file(&self, temp: &Path, final_path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.publish_file(temp, final_path)
    }
    fn note_publication(&self, session: &Path, publication: &str) -> Result<(), RestoreFault> {
        DirectoryQuarantine.note_publication(session, publication)
    }
    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }
    fn remove_publication(&self, session: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_publication(session)
    }

    fn remove_file(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_file(path)
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

    fn hold(
        &self,
        root: &AnchoredRoot,
    ) -> Result<Box<dyn crate::backup::application::RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }

    fn mark_ready(&self, root: &AnchoredRoot, staging_id: &str) -> Result<(), RestoreFault> {
        DirectoryQuarantine.mark_ready(root, staging_id)
    }

    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }

    fn spoil(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.spoil(path)
    }

    fn session_cut(
        &self,
        path: &Path,
        conversation_id: &str,
    ) -> Result<crate::backup::application::SessionCut, RestoreFault> {
        DirectoryQuarantine.session_cut(path, conversation_id)
    }
}

struct KeepsSession;

impl QuarantineLayout for KeepsSession {
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
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded)
    }

    fn remove_tree(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_tree(path)
    }

    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }

    fn session_temp(&self, root: &AnchoredRoot, relative: &str) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.session_temp(root, relative)
    }

    fn publish_file(&self, temp: &Path, final_path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.publish_file(temp, final_path)
    }
    fn note_publication(&self, session: &Path, publication: &str) -> Result<(), RestoreFault> {
        DirectoryQuarantine.note_publication(session, publication)
    }
    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }
    fn remove_publication(&self, session: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_publication(session)
    }

    fn remove_file(&self, path: &Path) -> Result<(), RestoreFault> {
        if path.extension().and_then(|ext| ext.to_str()) == Some("sqlite3") {
            return Err(RestoreFault::Unreadable);
        }
        DirectoryQuarantine.remove_file(path)
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

    fn hold(
        &self,
        root: &AnchoredRoot,
    ) -> Result<Box<dyn crate::backup::application::RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }

    fn mark_ready(&self, root: &AnchoredRoot, staging_id: &str) -> Result<(), RestoreFault> {
        DirectoryQuarantine.mark_ready(root, staging_id)
    }

    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }

    fn spoil(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.spoil(path)
    }

    fn session_cut(
        &self,
        path: &Path,
        conversation_id: &str,
    ) -> Result<crate::backup::application::SessionCut, RestoreFault> {
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
    assert_eq!(error, RestoreFault::Audit);
    assert!(place.quarantine.is_dir());
    assert!(!place.quarantine.join("journal.txt").exists());
    assert!(audit
        .lines()
        .iter()
        .all(|line| !(line.before == "verified" && line.after == "absent")));
    let accepted = accept(&place, &MemoryAudit::open(), &NoReceipts).unwrap_err();
    assert_eq!(accepted, RestoreFault::Unreadable);
    assert!(!place.live.exists());
}

struct SpendMissed {
    keep_marker: bool,
}

impl QuarantineLayout for SpendMissed {
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
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded)
    }

    fn remove_tree(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_tree(path)
    }

    fn remove_known(&self, _path: &Path, _identity: &TreeId) -> Result<(), RestoreFault> {
        Err(RestoreFault::Unreadable)
    }

    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }

    fn session_temp(&self, root: &AnchoredRoot, relative: &str) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.session_temp(root, relative)
    }

    fn publish_file(&self, temp: &Path, final_path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.publish_file(temp, final_path)
    }

    fn note_publication(&self, session: &Path, publication: &str) -> Result<(), RestoreFault> {
        DirectoryQuarantine.note_publication(session, publication)
    }

    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }

    fn remove_publication(&self, session: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_publication(session)
    }

    fn remove_file(&self, path: &Path) -> Result<(), RestoreFault> {
        if self.keep_marker
            && path.file_name().and_then(|name| name.to_str()) == Some("accept-ready")
        {
            return Err(RestoreFault::Unreadable);
        }
        DirectoryQuarantine.remove_file(path)
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

    fn hold(
        &self,
        root: &AnchoredRoot,
    ) -> Result<Box<dyn crate::backup::application::RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }

    fn mark_ready(&self, root: &AnchoredRoot, staging_id: &str) -> Result<(), RestoreFault> {
        DirectoryQuarantine.mark_ready(root, staging_id)?;
        Err(RestoreFault::Unreadable)
    }

    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }

    fn spoil(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.spoil(path)
    }

    fn session_cut(
        &self,
        path: &Path,
        conversation_id: &str,
    ) -> Result<crate::backup::application::SessionCut, RestoreFault> {
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
    assert_eq!(error, RestoreFault::Audit);
    assert!(place.quarantine.join("journal.txt").is_file());
    assert!(!place.quarantine.join("accept-ready").exists());
    assert!(audit.lines().iter().any(|line| line.cause == "sealed"));
    assert!(audit.lines().iter().all(|line| line.cause != "spent"));
    let accepted = accept(&place, &MemoryAudit::open(), &NoReceipts).unwrap_err();
    assert_eq!(accepted, RestoreFault::Tamper);
    assert!(!place.live.exists());
}

#[tokio::test]
async fn a_marker_that_survives_a_failed_spend_is_retained() {
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
fn a_partial_copy_removes_quarantine_and_leaves_live_data() {
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
    assert!(partial_names(place._keep.path()).is_empty());
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
    let loaded = DirectoryQuarantine.load(&quarantine).unwrap();
    let error = DirectoryQuarantine
        .promote(&quarantine, &live, &loaded)
        .unwrap_err();
    assert_eq!(error, RestoreFault::LiveOccupied);
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
    let loaded = DirectoryQuarantine.load(&quarantine).unwrap();
    let saved = place.quarantine.with_extension("saved");
    fs::rename(&place.quarantine, &saved).unwrap();
    fs::create_dir(&place.quarantine).unwrap();
    fs::write(place.quarantine.join("journal.txt"), b"other\n").unwrap();
    let error = DirectoryQuarantine
        .promote(&quarantine, &live, &loaded)
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
    let loaded = DirectoryQuarantine.load(&quarantine).unwrap();
    let mut held = fs::OpenOptions::new()
        .write(true)
        .open(place.quarantine.join("records/note"))
        .unwrap();
    DirectoryQuarantine
        .promote(&quarantine, &live, &loaded)
        .unwrap();
    use std::io::{Seek, SeekFrom, Write};
    held.seek(SeekFrom::Start(0)).unwrap();
    held.write_all(b"two").unwrap();
    held.sync_all().unwrap();
    assert_eq!(fs::read(place.live.join("records/note")).unwrap(), b"one");
    assert!(!place.live.join("accept-ready").exists());
}

#[test]
fn a_changed_quarantine_file_is_not_promoted() {
    let place = roots();
    fs::create_dir_all(place.quarantine.join("records")).unwrap();
    fs::write(place.quarantine.join("journal.txt"), b"sealed\n").unwrap();
    fs::write(place.quarantine.join("records/note"), b"one").unwrap();
    let quarantine = DirectoryQuarantine.anchor(&place.quarantine).unwrap();
    let live = DirectoryQuarantine.anchor(&place.live).unwrap();
    let loaded = DirectoryQuarantine.load(&quarantine).unwrap();
    fs::write(place.quarantine.join("records/note"), b"two").unwrap();
    let error = DirectoryQuarantine
        .promote(&quarantine, &live, &loaded)
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
    use crate::backup::domain::VolumeCase;
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
fn a_discarded_partial_directory_is_removed() {
    let place = roots();
    let quarantine = DirectoryQuarantine.anchor(&place.quarantine).unwrap();
    let live = DirectoryQuarantine.anchor(&place.live).unwrap();
    let mut staging = DirectoryQuarantine.begin(&quarantine, &live).unwrap();
    staging.write("records/notes/note-1", b"partial").unwrap();
    staging.discard().unwrap();
    assert!(partial_names(place._keep.path()).is_empty());
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
        line.before == "absent" && line.after == "unready" && line.cause == "retained"
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
    assert_eq!(error, RestoreFault::Unreadable);
    assert!(!removed.quarantine.exists());
    assert!(removed_audit.lines().iter().any(|line| {
        line.decision == "effect" && line.before == "unchanged" && line.after == "unchanged"
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
) -> Result<crate::backup::application::StageReport, RestoreFault> {
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
    fs::rename(&place.live, &place.quarantine).unwrap();
    fs::write(place.quarantine.join("accept-ready"), b"stage-fixed\n").unwrap();
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
    fs::write(place.quarantine.join("accept-ready"), b"ready\n").unwrap();
    let error = accept(&place, &audit, &NoReceipts).unwrap_err();
    assert_eq!(error, RestoreFault::Tamper);
    assert!(!place.live.exists());
}

#[tokio::test]
async fn quarantine_does_not_permit_replay() {
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
    assert_eq!(error, RestoreFault::Unreadable);
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
async fn a_failed_outcome_audit_discards_the_quarantine() {
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
    assert_eq!(error, RestoreFault::Audit);
    assert!(!place.quarantine.exists());
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

    fn finish(self: Box<Self>, _journal: &str) -> Result<TreeId, RestoreFault> {
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
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded)
    }
    fn remove_tree(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_tree(path)
    }
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }
    fn session_temp(&self, root: &AnchoredRoot, relative: &str) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.session_temp(root, relative)
    }
    fn publish_file(&self, temp: &Path, final_path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.publish_file(temp, final_path)
    }
    fn note_publication(&self, session: &Path, publication: &str) -> Result<(), RestoreFault> {
        DirectoryQuarantine.note_publication(session, publication)
    }
    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }
    fn remove_publication(&self, session: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_publication(session)
    }
    fn remove_file(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_file(path)
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
    fn hold(
        &self,
        root: &AnchoredRoot,
    ) -> Result<Box<dyn crate::backup::application::RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }
    fn mark_ready(&self, root: &AnchoredRoot, staging_id: &str) -> Result<(), RestoreFault> {
        DirectoryQuarantine.mark_ready(root, staging_id)
    }
    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }
    fn spoil(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.spoil(path)
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
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded)
    }
    fn remove_tree(&self, path: &Path) -> Result<(), RestoreFault> {
        if self.keep {
            return Err(RestoreFault::Unreadable);
        }
        DirectoryQuarantine.remove_tree(path)
    }
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }
    fn session_temp(&self, root: &AnchoredRoot, relative: &str) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.session_temp(root, relative)
    }
    fn publish_file(&self, temp: &Path, final_path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.publish_file(temp, final_path)
    }
    fn note_publication(&self, session: &Path, publication: &str) -> Result<(), RestoreFault> {
        DirectoryQuarantine.note_publication(session, publication)
    }
    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }
    fn remove_publication(&self, session: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_publication(session)
    }
    fn remove_file(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_file(path)
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
    fn hold(
        &self,
        root: &AnchoredRoot,
    ) -> Result<Box<dyn crate::backup::application::RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }
    fn mark_ready(&self, _root: &AnchoredRoot, _staging_id: &str) -> Result<(), RestoreFault> {
        Err(RestoreFault::Unreadable)
    }
    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }
    fn spoil(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.spoil(path)
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
    assert_eq!(error, RestoreFault::Unreadable);
    assert!(!removed.quarantine.exists());
    assert!(removed_audit.lines().iter().any(|line| {
        line.before == "verified" && line.after == "absent" && line.cause == "unreadable"
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
    assert_eq!(error, RestoreFault::Unreadable);
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
    assert_eq!(report.freshness, crate::backup::domain::Freshness::Superset);
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
    assert!(!place.quarantine.exists());
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
    let error = open_one(&place, "conv-bulk", &source, &MemoryAudit::open()).unwrap_err();
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
    let accepted = audit
        .lines()
        .into_iter()
        .find(|line| line.cause == "accepted")
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
        destination: &Path,
        progress: &mut dyn FetchProgress,
    ) -> Result<(), SessionSourceError> {
        *self.writes.lock().unwrap() += 1;
        progress.report(0, self.bytes.len() as u64);
        match self.mode {
            "missing" => Err(SessionSourceError::Missing),
            "interrupted" => {
                fs::write(destination, b"partial-session").unwrap();
                Err(SessionSourceError::Interrupted)
            }
            "changed" => {
                fs::write(destination, &self.other).unwrap();
                Ok(())
            }
            _ => {
                fs::write(destination, &self.bytes).unwrap();
                Ok(())
            }
        }
    }

    fn inspect(
        &self,
        conversation_id: &str,
        path: &Path,
    ) -> Result<SessionCut, SessionSourceError> {
        DirectoryQuarantine
            .session_cut(path, conversation_id)
            .map_err(|_| SessionSourceError::Unreadable)
    }
}

fn deferred_rows(conversation_id: &str, size: u64, digest: Fingerprint) -> Vec<DeferredEntry> {
    vec![
        DeferredEntry::new(StoreKind::CanonicalRecords, conversation_id, size, digest).unwrap(),
        DeferredEntry::new(StoreKind::Notes, conversation_id, size, digest).unwrap(),
    ]
}

async fn deferred_bundle(lines: Vec<DeferredEntry>) -> (Bundle, Roots) {
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
    (bundle, place)
}

fn mark_available(audit: &MemoryAudit, root: &Path, conversation_id: &str) {
    let anchored = DirectoryQuarantine.anchor(root).unwrap();
    let target = format!("{conversation_id}\n{}", anchored.display());
    let publication = "pub-current";
    let session = anchored.join(format!("sessions/{conversation_id}.sqlite3"));
    DirectoryQuarantine
        .note_publication(&session, publication)
        .unwrap();
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
        })
        .unwrap();
}

fn open_one(
    place: &Roots,
    conversation_id: &str,
    source: &dyn DeferredSessionSource,
    audit: &dyn RestoreAudit,
) -> Result<crate::backup::application::OpenReport, RestoreFault> {
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

#[tokio::test]
async fn an_opened_deferred_conversation_matches_the_recorded_fingerprint() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let source = FakeSource::matching(&session);
    let audit = MemoryAudit::open();
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
    let (_bundle, place) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let source = FakeSource::matching(&session);
    let audit = MemoryAudit::open();
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
    let (_bundle, place) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let source = FakeSource::matching(&session);
    let error = open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &place.live,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &MemoryAudit::open(),
        &DirectoryQuarantine,
        &source,
        &NoIds,
        &mut Quiet,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Unreadable);
    assert!(!place.live.join("sessions/conv-bulk.sqlite3").exists());
    assert!(partial_names(&place.live).is_empty());
}

#[tokio::test]
async fn a_changed_deferred_line_is_not_fetched() {
    let digest = Fingerprint::of(b"session-v1");
    let (_bundle, place) = deferred_bundle(deferred_rows("conv-bulk", 11, digest)).await;
    let path = place.live.join("deferred.txt");
    let mut text = fs::read_to_string(&path).unwrap();
    text = text.replace("11", "12");
    fs::write(&path, text).unwrap();
    let source = FakeSource::unused();
    let error = open_one(&place, "conv-bulk", &source, &MemoryAudit::open()).unwrap_err();
    assert_eq!(error, RestoreFault::Tamper);
    assert_eq!(*source.writes.lock().unwrap(), 0);
    assert!(!place.live.join("sessions/conv-bulk.sqlite3").exists());
}

#[tokio::test]
async fn a_failed_fetch_audit_removes_the_session() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let source = FakeSource::matching(&session);
    let error = open_one(&place, "conv-bulk", &source, &MemoryAudit::fail_after(1)).unwrap_err();
    assert_eq!(error, RestoreFault::Audit);
    assert!(!place.live.join("sessions/conv-bulk.sqlite3").exists());
    assert!(partial_names(&place.live).is_empty());
}

#[tokio::test]
async fn a_failed_fetch_cleanup_moves_the_session() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let source = FakeSource::matching(&session);
    let error = open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &place.live,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &MemoryAudit::fail_after(2),
        &KeepsSession,
        &source,
        &OsPublicationIds,
        &mut Quiet,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Audit);
    assert!(!place.live.join("sessions/conv-bulk.sqlite3").exists());
    assert!(place
        .live
        .join("sessions/conv-bulk.sqlite3.rejected")
        .is_file());
}

#[tokio::test]
async fn a_changed_source_is_refused_and_other_conversations_stay() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let other = place.live.join("records/notes/note-1");
    let before = fs::read(&other).unwrap();
    let mut source = FakeSource::matching(&session);
    source.mode = "changed";
    let error = open_one(&place, "conv-bulk", &source, &MemoryAudit::open()).unwrap_err();
    assert_eq!(error, RestoreFault::SourceChanged);
    assert!(!place.live.join("sessions/conv-bulk.sqlite3").is_file());
    assert!(partial_names(&place.live).is_empty());
    assert_eq!(fs::read(&other).unwrap(), before);
}

#[tokio::test]
async fn a_missing_source_is_refused_and_other_conversations_stay() {
    let digest = Fingerprint::of(b"session-v1");
    let (_bundle, place) = deferred_bundle(deferred_rows("conv-bulk", 11, digest)).await;
    let other = place.live.join("records/notes/note-1");
    let before = fs::read(&other).unwrap();
    let mut source = FakeSource::unused();
    source.mode = "missing";
    let error = open_one(&place, "conv-bulk", &source, &MemoryAudit::open()).unwrap_err();
    assert_eq!(error, RestoreFault::SourceMissing);
    assert_eq!(fs::read(&other).unwrap(), before);
    assert!(!place.live.join("sessions/conv-bulk.sqlite3").exists());
}

#[tokio::test]
async fn an_interrupted_fetch_leaves_no_partial_session() {
    let digest = Fingerprint::of(b"session-v1");
    let (_bundle, place) = deferred_bundle(deferred_rows("conv-bulk", 11, digest)).await;
    let mut source = FakeSource::unused();
    source.mode = "interrupted";
    let error = open_one(&place, "conv-bulk", &source, &MemoryAudit::open()).unwrap_err();
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
    let (_bundle, place) = deferred_bundle(deferred_rows("conv-bulk", 11, digest)).await;
    let mut source = FakeSource::unused();
    source.mode = "interrupted";
    let audit = MemoryAudit::open();
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
    let later = open_one(&place, "conv-bulk", &source, &MemoryAudit::open()).unwrap_err();
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

fn session_partial(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".partial"))
}

/// Removal, the move aside, and emptying all fail for a `.partial` session.
struct KeepsPartialSession;

impl QuarantineLayout for KeepsPartialSession {
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
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded)
    }
    fn remove_tree(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_tree(path)
    }
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }
    fn session_temp(&self, root: &AnchoredRoot, relative: &str) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.session_temp(root, relative)
    }
    fn publish_file(&self, temp: &Path, final_path: &Path) -> Result<(), RestoreFault> {
        if session_partial(temp) {
            return Err(RestoreFault::Rename);
        }
        DirectoryQuarantine.publish_file(temp, final_path)
    }
    fn note_publication(&self, session: &Path, publication: &str) -> Result<(), RestoreFault> {
        DirectoryQuarantine.note_publication(session, publication)
    }
    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }
    fn remove_publication(&self, session: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_publication(session)
    }
    fn remove_file(&self, path: &Path) -> Result<(), RestoreFault> {
        if session_partial(path) && path.exists() {
            return Err(RestoreFault::Unreadable);
        }
        DirectoryQuarantine.remove_file(path)
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
    fn hold(
        &self,
        root: &AnchoredRoot,
    ) -> Result<Box<dyn crate::backup::application::RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }
    fn mark_ready(&self, root: &AnchoredRoot, staging_id: &str) -> Result<(), RestoreFault> {
        DirectoryQuarantine.mark_ready(root, staging_id)
    }
    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }
    fn spoil(&self, path: &Path) -> Result<(), RestoreFault> {
        if session_partial(path) {
            return Err(RestoreFault::Unreadable);
        }
        DirectoryQuarantine.spoil(path)
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

    fn finish(self: Box<Self>, _journal: &str) -> Result<TreeId, RestoreFault> {
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
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded)
    }
    fn remove_tree(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_tree(path)
    }
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }
    fn session_temp(&self, root: &AnchoredRoot, relative: &str) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.session_temp(root, relative)
    }
    fn publish_file(&self, temp: &Path, final_path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.publish_file(temp, final_path)
    }
    fn note_publication(&self, session: &Path, publication: &str) -> Result<(), RestoreFault> {
        DirectoryQuarantine.note_publication(session, publication)
    }
    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }
    fn remove_publication(&self, session: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_publication(session)
    }
    fn remove_file(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_file(path)
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
    fn hold(
        &self,
        root: &AnchoredRoot,
    ) -> Result<Box<dyn crate::backup::application::RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }
    fn mark_ready(&self, root: &AnchoredRoot, staging_id: &str) -> Result<(), RestoreFault> {
        DirectoryQuarantine.mark_ready(root, staging_id)
    }
    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }
    fn spoil(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.spoil(path)
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
        line.before == "absent" && line.after == "unready" && line.cause == "retained"
    }));
    assert!(audit.lines().iter().all(|line| line.after != "unchanged"));
    assert!(!place.live.exists());
    assert!(!place.quarantine.exists());
}

#[tokio::test]
async fn an_unknown_conversation_is_not_fetched() {
    let digest = Fingerprint::of(b"session-v1");
    let (_bundle, place) = deferred_bundle(deferred_rows("conv-bulk", 11, digest)).await;
    let source = FakeSource::unused();
    let error = open_one(&place, "conv-other", &source, &MemoryAudit::open()).unwrap_err();
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
    let (_bundle, place) = deferred_bundle(lines).await;
    let source = FakeSource::unused();
    let error = open_one(&place, "conv-bulk", &source, &MemoryAudit::open()).unwrap_err();
    assert_eq!(error, RestoreFault::Fingerprint);
    assert_eq!(*source.writes.lock().unwrap(), 0);
}

#[tokio::test]
async fn a_matching_session_is_not_fetched_again() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let path = place.live.join("sessions/conv-bulk.sqlite3");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, &session.bytes).unwrap();
    let source = FakeSource::matching(&session);
    let audit = MemoryAudit::open();
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
    let (_bundle, place) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let path = place.live.join("sessions/conv-bulk.sqlite3");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, &other.bytes).unwrap();
    let source = FakeSource::matching(&session);
    let audit = MemoryAudit::open();
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
    assert!(!opened.already_present);
    assert_eq!(opened.size, bulk_len as u64);
    let session = place.live.join("sessions/conv-bulk.sqlite3");
    let inspected = source.inspect("conv-bulk", &session).unwrap();
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
    let again = source.inspect("conv-bulk", &session).unwrap();
    assert_eq!(again, inspected);
    assert!(!place.live.join("sessions/conv-kept.sqlite3").exists());

    let record = fs::read_to_string(audit_dir.join("restore.jsonl")).unwrap();
    assert!(record.contains("\"cause\":\"sealed\""));
    assert!(record.contains("\"cause\":\"accepted\""));
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
            assert!(!quarantine.exists());
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
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded)
    }
    fn remove_tree(&self, _path: &Path) -> Result<(), RestoreFault> {
        Err(RestoreFault::Unreadable)
    }
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }
    fn session_temp(&self, root: &AnchoredRoot, relative: &str) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.session_temp(root, relative)
    }
    fn publish_file(&self, temp: &Path, final_path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.publish_file(temp, final_path)
    }
    fn note_publication(&self, session: &Path, publication: &str) -> Result<(), RestoreFault> {
        DirectoryQuarantine.note_publication(session, publication)
    }
    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }
    fn remove_publication(&self, session: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_publication(session)
    }
    fn remove_file(&self, path: &Path) -> Result<(), RestoreFault> {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if name.ends_with(".partial") {
            return DirectoryQuarantine.remove_file(path);
        }
        Err(RestoreFault::Unreadable)
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
    fn hold(
        &self,
        root: &AnchoredRoot,
    ) -> Result<Box<dyn crate::backup::application::RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }
    fn mark_ready(&self, root: &AnchoredRoot, staging_id: &str) -> Result<(), RestoreFault> {
        DirectoryQuarantine.mark_ready(root, staging_id)
    }
    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }
    fn spoil(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.spoil(path)
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
    assert_eq!(error, RestoreFault::Audit);
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
    let (_bundle, place) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let source = FakeSource::matching(&session);
    let error = open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &place.live,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &MemoryAudit::fail_after(2),
        &KeepsTree,
        &source,
        &OsPublicationIds,
        &mut Quiet,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Audit);
    let path = place.live.join("sessions/conv-bulk.sqlite3");
    assert_eq!(fs::metadata(&path).unwrap().len(), 0);
    assert!(!path.with_extension("sqlite3.rejected").exists());
}

/// Remove, rename, and empty all fail once the session file exists.
struct KeepsBytes;

impl QuarantineLayout for KeepsBytes {
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
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded)
    }
    fn remove_tree(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_tree(path)
    }
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }
    fn session_temp(&self, root: &AnchoredRoot, relative: &str) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.session_temp(root, relative)
    }
    fn publish_file(&self, temp: &Path, final_path: &Path) -> Result<(), RestoreFault> {
        let name = final_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if name.ends_with(".rejected") {
            return Err(RestoreFault::Unreadable);
        }
        DirectoryQuarantine.publish_file(temp, final_path)
    }
    fn note_publication(&self, session: &Path, publication: &str) -> Result<(), RestoreFault> {
        DirectoryQuarantine.note_publication(session, publication)
    }
    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }
    fn remove_publication(&self, _session: &Path) -> Result<(), RestoreFault> {
        Err(RestoreFault::Unreadable)
    }
    fn remove_file(&self, path: &Path) -> Result<(), RestoreFault> {
        if path.exists() {
            return Err(RestoreFault::Unreadable);
        }
        Ok(())
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
    fn hold(
        &self,
        root: &AnchoredRoot,
    ) -> Result<Box<dyn crate::backup::application::RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }
    fn mark_ready(&self, root: &AnchoredRoot, staging_id: &str) -> Result<(), RestoreFault> {
        DirectoryQuarantine.mark_ready(root, staging_id)
    }
    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }
    fn spoil(&self, _path: &Path) -> Result<(), RestoreFault> {
        Err(RestoreFault::Unreadable)
    }
    fn session_cut(&self, path: &Path, conversation_id: &str) -> Result<SessionCut, RestoreFault> {
        DirectoryQuarantine.session_cut(path, conversation_id)
    }
}

#[tokio::test]
async fn a_failed_fetch_that_cannot_be_retired_is_not_served() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let source = FakeSource::matching(&session);
    let error = open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &place.live,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &MemoryAudit::fail_after(2),
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
        &MemoryAudit::open(),
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
        .expect("the failed fetch still wrote its publication");
    assert_ne!(publication, "pub-earlier");
    let earlier = MemoryAudit::open();
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
    assert!(!place.quarantine.exists());
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
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded)?;
        Ok(Promoted { durable: false })
    }
    fn remove_tree(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_tree(path)
    }
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }
    fn session_temp(&self, root: &AnchoredRoot, relative: &str) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.session_temp(root, relative)
    }
    fn publish_file(&self, temp: &Path, final_path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.publish_file(temp, final_path)
    }
    fn note_publication(&self, session: &Path, publication: &str) -> Result<(), RestoreFault> {
        DirectoryQuarantine.note_publication(session, publication)
    }
    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }
    fn remove_publication(&self, session: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_publication(session)
    }
    fn remove_file(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_file(path)
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
    fn hold(
        &self,
        root: &AnchoredRoot,
    ) -> Result<Box<dyn crate::backup::application::RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }
    fn mark_ready(&self, root: &AnchoredRoot, staging_id: &str) -> Result<(), RestoreFault> {
        DirectoryQuarantine.mark_ready(root, staging_id)
    }
    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }
    fn spoil(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.spoil(path)
    }
    fn session_cut(&self, path: &Path, conversation_id: &str) -> Result<SessionCut, RestoreFault> {
        DirectoryQuarantine.session_cut(path, conversation_id)
    }
}

#[tokio::test]
async fn an_alias_of_the_live_root_reuses_the_fetched_session() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let mut source = FakeSource::matching(&session);
    let audit = MemoryAudit::open();
    let first = open_one(&place, "conv-bulk", &source, &audit).unwrap();
    assert!(!first.already_present);
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
    let path = place.live.join("sessions/conv-bulk.sqlite3");
    assert_eq!(fs::read(&path).unwrap(), session.bytes);
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
    accept(&place, &audit, &NoReceipts).unwrap();
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
        session.bytes
    );
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
        &MemoryAudit::fail_after(2),
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
struct ExistsFailsAfterPublish {
    published: AtomicBool,
}

impl QuarantineLayout for ExistsFailsAfterPublish {
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
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded)
    }
    fn remove_tree(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_tree(path)
    }
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }
    fn session_temp(&self, root: &AnchoredRoot, relative: &str) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.session_temp(root, relative)
    }
    fn publish_file(&self, temp: &Path, final_path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.publish_file(temp, final_path)?;
        self.published.store(true, Ordering::Relaxed);
        Err(RestoreFault::Unreadable)
    }
    fn note_publication(&self, session: &Path, publication: &str) -> Result<(), RestoreFault> {
        DirectoryQuarantine.note_publication(session, publication)
    }
    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }
    fn remove_publication(&self, session: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_publication(session)
    }
    fn remove_file(&self, path: &Path) -> Result<(), RestoreFault> {
        if self.published.load(Ordering::Relaxed) && !path.exists() {
            return Err(RestoreFault::Unreadable);
        }
        DirectoryQuarantine.remove_file(path)
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
    fn hold(
        &self,
        root: &AnchoredRoot,
    ) -> Result<Box<dyn crate::backup::application::RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }
    fn mark_ready(&self, root: &AnchoredRoot, staging_id: &str) -> Result<(), RestoreFault> {
        DirectoryQuarantine.mark_ready(root, staging_id)
    }
    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }
    fn spoil(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.spoil(path)
    }
    fn session_cut(&self, path: &Path, conversation_id: &str) -> Result<SessionCut, RestoreFault> {
        DirectoryQuarantine.session_cut(path, conversation_id)
    }
}

#[tokio::test]
async fn a_failed_publish_retires_the_final_name_when_the_temp_is_already_gone() {
    let session = sealed_session(b"session-v1");
    let (_bundle, place) =
        deferred_bundle(deferred_rows("conv-bulk", session.size, session.digest)).await;
    let source = FakeSource::matching(&session);
    let layout = ExistsFailsAfterPublish {
        published: AtomicBool::new(false),
    };
    let error = open_deferred_conversation(
        &OpenRequest {
            facts: person(RestoreAction::Fetch),
            root: &place.live,
            conversation_id: "conv-bulk",
        },
        &policy(),
        &MemoryAudit::open(),
        &layout,
        &source,
        &OsPublicationIds,
        &mut Quiet,
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Retained);
    assert!(!place.live.join("sessions/conv-bulk.sqlite3").exists());
    assert!(partial_names(&place.live).is_empty());
}

struct RetainedStaging {
    quarantine: PathBuf,
}

impl Staging for RetainedStaging {
    fn write(&mut self, _relative: &str, _bytes: &[u8]) -> Result<(), RestoreFault> {
        Ok(())
    }

    fn finish(self: Box<Self>, _journal: &str) -> Result<TreeId, RestoreFault> {
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
    ) -> Result<Promoted, RestoreFault> {
        DirectoryQuarantine.promote(quarantine, live, loaded)
    }
    fn remove_tree(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_tree(path)
    }
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        DirectoryQuarantine.read_bytes(path)
    }
    fn session_temp(&self, root: &AnchoredRoot, relative: &str) -> Result<PathBuf, RestoreFault> {
        DirectoryQuarantine.session_temp(root, relative)
    }
    fn publish_file(&self, temp: &Path, final_path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.publish_file(temp, final_path)
    }
    fn note_publication(&self, session: &Path, publication: &str) -> Result<(), RestoreFault> {
        DirectoryQuarantine.note_publication(session, publication)
    }
    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.publication(session)
    }
    fn remove_publication(&self, session: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_publication(session)
    }
    fn remove_file(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.remove_file(path)
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
    fn hold(
        &self,
        root: &AnchoredRoot,
    ) -> Result<Box<dyn crate::backup::application::RestoreHold>, RestoreFault> {
        DirectoryQuarantine.hold(root)
    }
    fn mark_ready(&self, root: &AnchoredRoot, staging_id: &str) -> Result<(), RestoreFault> {
        DirectoryQuarantine.mark_ready(root, staging_id)
    }
    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        DirectoryQuarantine.ready(root)
    }
    fn spoil(&self, path: &Path) -> Result<(), RestoreFault> {
        DirectoryQuarantine.spoil(path)
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
