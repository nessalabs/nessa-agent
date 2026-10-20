//! Quarantine, acceptance, and on-demand fetch of a deferred conversation.
//!
//! A failed or partial stage leaves the live path untouched. Acceptance is a
//! rename onto an empty live path. An oversized conversation is fetched only
//! when it is opened, and only when its stream still matches the cut.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;

use crate::backup::application::Authenticator;
use crate::backup::application::{
    accept_quarantine, open_deferred_conversation, root_posture, stage_quarantine, AcceptRequest,
    DecisionAuditError, DeferredSessionSource, ExportClock, FetchProgress, InventoryCause,
    InventoryDecision, LeaseReceipts, MacKey, ManifestPublisher, MutationAdmission, OpenRequest,
    QuarantineLayout, RestoreAction, RestoreAudit, RestoreEvidence, RestoreFacts,
    RestorePolicyEvaluator, SessionCut, SessionSourceError, StageRequest, VerificationLimits,
};
use crate::backup::domain::{
    AuthenticatedManifest, CapturedRecord, CredentialKind, DeferredEntry, DeletionFence,
    DeletionLog, Fingerprint, GrantRole, Manifest, Posture, RestoreFault, Sensitivity, SourceCut,
    StoreKind, Watermark, WRITER_INVENTORY,
};
use crate::backup::infrastructure::snapshot::SESSION_EXPORT_LIMIT;
use crate::backup::infrastructure::{
    capture_namespace, CedarInventoryPolicy, DirectoryMaterializer, DirectoryPublisher,
    DirectoryQuarantine, DirectoryVerificationSource, ExportFormat, FileDeletionLog,
    FileRestoreAudit, KeyedInventory, SqliteDeferredSource,
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
    before: String,
    after: String,
    cause: String,
    initiator: String,
    decision: String,
    policies: Vec<String>,
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
            before: evidence.before.to_owned(),
            after: evidence.after.to_owned(),
            cause: evidence.cause.to_owned(),
            initiator: evidence.initiator.to_owned(),
            decision: evidence.decision.to_owned(),
            policies: evidence.policies.to_vec(),
        });
        Ok(())
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
    )
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
        let mut staging = DirectoryQuarantine
            .begin(&place.quarantine, &place.live)
            .unwrap();
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
}

struct FakeSource {
    size: u64,
    digest: Fingerprint,
    mode: &'static str,
    writes: Mutex<u32>,
}

impl FakeSource {
    fn matching(size: u64, digest: Fingerprint) -> Self {
        Self {
            size,
            digest,
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
        progress.report(0, self.size);
        match self.mode {
            "missing" => Err(SessionSourceError::Missing),
            "interrupted" => {
                fs::write(destination, b"partial-session").unwrap();
                Err(SessionSourceError::Interrupted)
            }
            _ => {
                fs::write(destination, b"session").unwrap();
                Ok(())
            }
        }
    }

    fn inspect(
        &self,
        _conversation_id: &str,
        _path: &Path,
    ) -> Result<SessionCut, SessionSourceError> {
        let digest = if self.mode == "changed" || self.mode == "differs" {
            Fingerprint::of(b"other")
        } else {
            self.digest
        };
        let size = if self.mode == "changed" || self.mode == "differs" {
            self.size + 1
        } else {
            self.size
        };
        Ok(SessionCut { size, digest })
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
        &mut Quiet,
    )
}

#[tokio::test]
async fn an_opened_deferred_conversation_matches_the_recorded_fingerprint() {
    let digest = Fingerprint::of(b"session-v1");
    let (_bundle, place) = deferred_bundle(deferred_rows("conv-bulk", 11, digest)).await;
    let source = FakeSource::matching(11, digest);
    let audit = MemoryAudit::open();
    let report = open_one(&place, "conv-bulk", &source, &audit).unwrap();
    assert_eq!(report.size, 11);
    assert_eq!(report.digest, digest);
    assert!(!report.already_present);
    assert_eq!(*source.writes.lock().unwrap(), 1);
    assert!(place.live.join("sessions/conv-bulk.sqlite3").is_file());
    let fetched = audit
        .lines()
        .into_iter()
        .find(|line| line.cause == "fetched")
        .unwrap();
    assert_eq!(fetched.before, "deferred");
    assert_eq!(fetched.after, "available");
    assert_eq!(fetched.initiator, "person");
    assert_eq!(fetched.decision, "effect");
}

#[tokio::test]
async fn a_changed_source_is_refused_and_other_conversations_stay() {
    let digest = Fingerprint::of(b"session-v1");
    let (_bundle, place) = deferred_bundle(deferred_rows("conv-bulk", 11, digest)).await;
    let other = place.live.join("sessions/conv-kept.sqlite3");
    nessa_local_storage::create_directory(other.parent().unwrap()).unwrap();
    fs::write(&other, b"kept-session").unwrap();
    let mut source = FakeSource::matching(11, digest);
    source.mode = "changed";
    let error = open_one(&place, "conv-bulk", &source, &MemoryAudit::open()).unwrap_err();
    assert_eq!(error, RestoreFault::SourceChanged);
    assert!(!place.live.join("sessions/conv-bulk.sqlite3").is_file());
    assert!(partial_names(&place.live).is_empty());
    assert_eq!(fs::read(&other).unwrap(), b"kept-session");
}

#[tokio::test]
async fn a_missing_source_is_refused_and_other_conversations_stay() {
    let digest = Fingerprint::of(b"session-v1");
    let (_bundle, place) = deferred_bundle(deferred_rows("conv-bulk", 11, digest)).await;
    let other = place.live.join("sessions/conv-kept.sqlite3");
    nessa_local_storage::create_directory(other.parent().unwrap()).unwrap();
    fs::write(&other, b"kept-session").unwrap();
    let mut source = FakeSource::matching(11, digest);
    source.mode = "missing";
    let error = open_one(&place, "conv-bulk", &source, &MemoryAudit::open()).unwrap_err();
    assert_eq!(error, RestoreFault::SourceMissing);
    assert_eq!(fs::read(&other).unwrap(), b"kept-session");
    assert!(!place.live.join("sessions/conv-bulk.sqlite3").exists());
}

#[tokio::test]
async fn an_interrupted_fetch_leaves_no_partial_session() {
    let digest = Fingerprint::of(b"session-v1");
    let (_bundle, place) = deferred_bundle(deferred_rows("conv-bulk", 11, digest)).await;
    let mut source = FakeSource::matching(11, digest);
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
async fn an_unknown_conversation_is_not_fetched() {
    let digest = Fingerprint::of(b"session-v1");
    let (_bundle, place) = deferred_bundle(deferred_rows("conv-bulk", 11, digest)).await;
    let source = FakeSource::matching(11, digest);
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
    let source = FakeSource::matching(11, Fingerprint::of(b"one"));
    let error = open_one(&place, "conv-bulk", &source, &MemoryAudit::open()).unwrap_err();
    assert_eq!(error, RestoreFault::Fingerprint);
    assert_eq!(*source.writes.lock().unwrap(), 0);
}

#[tokio::test]
async fn a_matching_session_is_not_fetched_again() {
    let digest = Fingerprint::of(b"session-v1");
    let (_bundle, place) = deferred_bundle(deferred_rows("conv-bulk", 11, digest)).await;
    let path = place.live.join("sessions/conv-bulk.sqlite3");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"already").unwrap();
    let source = FakeSource::matching(11, digest);
    let report = open_one(&place, "conv-bulk", &source, &MemoryAudit::open()).unwrap();
    assert!(report.already_present);
    assert_eq!(*source.writes.lock().unwrap(), 0);
    assert_eq!(fs::read(&path).unwrap(), b"already");
}

#[tokio::test]
async fn a_different_session_file_is_not_replaced() {
    let digest = Fingerprint::of(b"session-v1");
    let (_bundle, place) = deferred_bundle(deferred_rows("conv-bulk", 11, digest)).await;
    let path = place.live.join("sessions/conv-bulk.sqlite3");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"do-not-replace").unwrap();
    let mut source = FakeSource::matching(11, digest);
    source.mode = "differs";
    let error = open_one(&place, "conv-bulk", &source, &MemoryAudit::open()).unwrap_err();
    assert_eq!(error, RestoreFault::SessionDiffers);
    assert_eq!(*source.writes.lock().unwrap(), 0);
    assert_eq!(fs::read(&path).unwrap(), b"do-not-replace");
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
