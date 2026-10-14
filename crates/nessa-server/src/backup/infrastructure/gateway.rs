//! Read one namespace into the writer inventory.
//!
//! Admission is held only while each store is pinned: a SQLite read snapshot
//! for a database, a copy for a small file. Records are read from those
//! snapshots after admission is released. A local outbox is not read. Secret
//! paths are fingerprinted. The credential registry contributes public facts
//! only.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;

use super::credentials::{read_credentials, CredentialRead, CredentialReadError};
use super::jsonl::jsonl_cut;
use super::sensitivity::path_sensitivity;
use super::snapshot::{DeferredStream, SnapshotCopy, SqliteSnapshot};
use crate::backup::application::{
    capture_export, Authenticator, CopiedStore, DeletionLogStore, ExportClock, ExportFailure,
    ExportRequest, ExportSource, FenceSource, ManifestPublisher, MutationAdmission, PinnedStore,
};
use crate::backup::domain::{
    CapturedRecord, CredentialKind, DeferredEntry, DeletionFence, Fingerprint, GrantRole,
    Sensitivity, SkipReason, SkippedRecord, SourceCut, StoreKind,
};
use crate::conversation::infrastructure::{
    ExportFences, ExportSkip, MetadataSnapshot, NamedRecord, ReceiverSnapshot,
    ReceiverSnapshotError,
};

/// Hold admission, read `namespace`, and publish the cut.
///
/// `authenticator` is the deletion inventory's mac key when the caller wants
/// one key for both. The key is not written into the bundle.
pub async fn capture_namespace(
    namespace: &Path,
    admission: &dyn MutationAdmission,
    inventory: &dyn DeletionLogStore,
    publisher: &dyn ManifestPublisher,
    clock: &dyn ExportClock,
    authenticator: &dyn Authenticator,
) -> Result<crate::backup::domain::AuthenticatedManifest, ExportFailure> {
    let facts = Arc::new(FactsCache::new(
        namespace.join("conversations").join("metadata.sqlite3"),
    ));
    let canonical = Arc::new(SessionFiles::new(
        namespace
            .join("conversations")
            .join("sessions")
            .join("records.sqlite3"),
    ));
    let credentials = Arc::new(CredentialCache::new(
        namespace.join("auth").join("credentials.v1.json"),
    ));
    let inventory_cut = InventoryCut { log: inventory };
    let fences = NamespaceFences {
        facts: facts.clone(),
        credentials: credentials.clone(),
    };
    let readers: Vec<Box<dyn ExportSource>> = vec![
        Box::new(CanonicalFile {
            files: canonical.clone(),
        }),
        Box::new(MetadataRows {
            facts: facts.clone(),
        }),
        Box::new(GrantRows {
            facts: facts.clone(),
        }),
        Box::new(NotesFile {
            files: canonical.clone(),
        }),
        Box::new(DirectoryStore {
            kind: StoreKind::DriveToolAudit,
            roots: vec![namespace
                .join("conversations")
                .join("audit")
                .join("drive-tools")],
            extra_files: vec![],
        }),
        Box::new(DirectoryStore {
            kind: StoreKind::EnvironmentAudit,
            roots: vec![
                namespace
                    .join("conversations")
                    .join("audit")
                    .join("warm-up"),
                namespace
                    .join("conversations")
                    .join("audit")
                    .join("environments"),
            ],
            extra_files: vec![namespace.join("environment").join("grants-audit.jsonl")],
        }),
        Box::new(DirectoryStore {
            kind: StoreKind::ConversationAudit,
            roots: vec![
                namespace
                    .join("conversations")
                    .join("audit")
                    .join("creation"),
                namespace
                    .join("conversations")
                    .join("audit")
                    .join("deletion"),
                namespace
                    .join("conversations")
                    .join("audit")
                    .join("approval-mode"),
                namespace
                    .join("conversations")
                    .join("audit")
                    .join("file-links"),
                namespace
                    .join("conversations")
                    .join("audit")
                    .join("mcp-apps"),
                namespace
                    .join("conversations")
                    .join("audit")
                    .join("execution"),
            ],
            extra_files: vec![],
        }),
        Box::new(DirectoryStore {
            kind: StoreKind::PeerPairings,
            roots: vec![
                namespace.join("peer-gateways"),
                namespace.join("peer-gateways-audit"),
            ],
            extra_files: vec![],
        }),
        Box::new(DirectoryStore {
            kind: StoreKind::EnvironmentPeerPairings,
            roots: vec![namespace.join("environment").join("pairing")],
            extra_files: vec![namespace.join("environment").join("paired-with")],
        }),
        Box::new(DirectoryStore {
            kind: StoreKind::WorkerEnrollments,
            roots: vec![
                namespace.join("workers"),
                namespace.join("environment").join("workers"),
            ],
            extra_files: vec![],
        }),
        Box::new(JsonFile {
            kind: StoreKind::EnvironmentGrants,
            path: namespace.join("environment").join("grants.json"),
        }),
        Box::new(RelayStore {
            namespace: namespace.to_path_buf(),
        }),
        Box::new(JsonlFile {
            kind: StoreKind::LeaseRecords,
            path: namespace.join("environment").join("leases.jsonl"),
        }),
        Box::new(DirectoryStore {
            kind: StoreKind::ArtifactHolds,
            roots: vec![namespace.join("attachments").join("holds")],
            extra_files: vec![],
        }),
        Box::new(CredentialStore {
            kind: StoreKind::DeviceCredentials,
            credentials: credentials.clone(),
            wanted: Some(CredentialKind::Device),
        }),
        Box::new(CredentialStore {
            kind: StoreKind::PeerCredentials,
            credentials: credentials.clone(),
            wanted: Some(CredentialKind::Peer),
        }),
        Box::new(CredentialStore {
            kind: StoreKind::EnvironmentPeerCredentials,
            credentials: credentials.clone(),
            wanted: Some(CredentialKind::EnvironmentPeer),
        }),
        Box::new(CredentialStore {
            kind: StoreKind::WorkerCredentials,
            credentials: credentials.clone(),
            wanted: None,
        }),
        Box::new(Receivers::new(
            namespace
                .join("receiver-access")
                .join("receiver-access.sqlite3"),
        )),
        Box::new(inventory_cut),
    ];
    let source_refs: Vec<&dyn ExportSource> =
        readers.iter().map(|reader| reader.as_ref()).collect();
    let fence_refs: Vec<&dyn FenceSource> = vec![&fences];
    capture_export(ExportRequest {
        admission,
        sources: &source_refs,
        fences: &fence_refs,
        inventory,
        publisher,
        clock,
        authenticator,
    })
    .await
}

struct FactsCache {
    path: PathBuf,
    state: Mutex<FactsState>,
}

enum FactsState {
    Cold,
    Absent,
    Open(MetadataSnapshot),
    Ready(crate::conversation::infrastructure::ExportFacts),
    Failed,
}

impl FactsCache {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            state: Mutex::new(FactsState::Cold),
        }
    }

    /// Open a read snapshot and record `head:journal`. Idempotent.
    async fn pin(&self) -> Result<(), ()> {
        {
            let guard = self.state.lock().map_err(|_| ())?;
            match &*guard {
                FactsState::Cold => {}
                FactsState::Failed => return Err(()),
                _ => return Ok(()),
            }
        }
        if !self.path.is_file() {
            *self.state.lock().map_err(|_| ())? = FactsState::Absent;
            return Ok(());
        }
        let path = self.path.clone();
        let opened = tokio::task::spawn_blocking(move || MetadataSnapshot::open(&path))
            .await
            .map_err(|_| ())?;
        let mut guard = self.state.lock().map_err(|_| ())?;
        if !matches!(*guard, FactsState::Cold) {
            return match &*guard {
                FactsState::Failed => Err(()),
                _ => Ok(()),
            };
        }
        match opened {
            Ok(snapshot) => {
                *guard = FactsState::Open(snapshot);
                Ok(())
            }
            Err(_) => {
                *guard = FactsState::Failed;
                Err(())
            }
        }
    }

    /// Tombstones and revocations from the snapshot [`Self::pin`] opened.
    async fn fence_facts(&self) -> Result<Option<ExportFences>, ()> {
        let snapshot = {
            let mut guard = self.state.lock().map_err(|_| ())?;
            match std::mem::replace(&mut *guard, FactsState::Cold) {
                FactsState::Absent => {
                    *guard = FactsState::Absent;
                    return Ok(None);
                }
                FactsState::Open(snapshot) => snapshot,
                FactsState::Ready(facts) => {
                    let fences = ExportFences {
                        tombstones: facts.tombstones.clone(),
                        revocations: facts.revocations.clone(),
                    };
                    *guard = FactsState::Ready(facts);
                    return Ok(Some(fences));
                }
                other => {
                    *guard = other;
                    return Err(());
                }
            }
        };
        let (snapshot, fences) = tokio::task::spawn_blocking(move || {
            let fences = snapshot.fences();
            (snapshot, fences)
        })
        .await
        .map_err(|_| ())?;
        match fences {
            Ok(fences) => {
                *self.state.lock().map_err(|_| ())? = FactsState::Open(snapshot);
                Ok(Some(fences))
            }
            Err(_) => {
                *self.state.lock().map_err(|_| ())? = FactsState::Failed;
                Err(())
            }
        }
    }

    /// Every metadata and grant row from the snapshot [`Self::pin`] opened.
    async fn load(&self) -> Result<Option<crate::conversation::infrastructure::ExportFacts>, ()> {
        let snapshot = {
            let mut guard = self.state.lock().map_err(|_| ())?;
            match std::mem::replace(&mut *guard, FactsState::Cold) {
                FactsState::Absent => {
                    *guard = FactsState::Absent;
                    return Ok(None);
                }
                FactsState::Ready(facts) => {
                    *guard = FactsState::Ready(facts.clone());
                    return Ok(Some(facts));
                }
                FactsState::Open(snapshot) => snapshot,
                other => {
                    *guard = other;
                    return Err(());
                }
            }
        };
        let facts = tokio::task::spawn_blocking(move || snapshot.facts())
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?;
        *self.state.lock().map_err(|_| ())? = FactsState::Ready(facts.clone());
        Ok(Some(facts))
    }
}

struct SessionFiles {
    path: PathBuf,
    state: Mutex<SessionState>,
}

enum SessionState {
    Cold,
    Absent,
    Unreadable,
    Deferred(Vec<DeferredStream>),
    Open(SqliteSnapshot),
    Bytes(Arc<Vec<u8>>),
}

enum SessionView {
    Absent,
    Unreadable,
    Deferred(Vec<DeferredStream>),
    Bytes(Arc<Vec<u8>>),
}

impl SessionFiles {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            state: Mutex::new(SessionState::Cold),
        }
    }

    /// Open a read snapshot when the file is a database. A missing file is
    /// absent. A file that is not a database is copied here, while admission
    /// is held, which is the same cut a raw read used to take.
    async fn pin(&self) -> Result<(), ()> {
        {
            let guard = self.state.lock().map_err(|_| ())?;
            if !matches!(*guard, SessionState::Cold) {
                return Ok(());
            }
        }
        if !self.path.exists() {
            *self.state.lock().map_err(|_| ())? = SessionState::Absent;
            return Ok(());
        }
        let path = self.path.clone();
        let opened = tokio::task::spawn_blocking(move || match SqliteSnapshot::open(&path) {
            Ok(snapshot) => SessionState::Open(snapshot),
            Err(()) => match fs::read(&path) {
                Ok(bytes) => SessionState::Bytes(Arc::new(bytes)),
                Err(_) => SessionState::Unreadable,
            },
        })
        .await
        .map_err(|_| ())?;
        let mut guard = self.state.lock().map_err(|_| ())?;
        if matches!(*guard, SessionState::Cold) {
            *guard = opened;
        }
        Ok(())
    }

    /// Bytes of the snapshot. The live file is not read again.
    async fn view(&self) -> Result<SessionView, ()> {
        let snapshot = {
            let mut guard = self.state.lock().map_err(|_| ())?;
            match std::mem::replace(&mut *guard, SessionState::Cold) {
                SessionState::Absent => {
                    *guard = SessionState::Absent;
                    return Ok(SessionView::Absent);
                }
                SessionState::Unreadable => {
                    *guard = SessionState::Unreadable;
                    return Ok(SessionView::Unreadable);
                }
                SessionState::Deferred(streams) => {
                    let shared = streams.clone();
                    *guard = SessionState::Deferred(streams);
                    return Ok(SessionView::Deferred(shared));
                }
                SessionState::Bytes(bytes) => {
                    let shared = Arc::clone(&bytes);
                    *guard = SessionState::Bytes(bytes);
                    return Ok(SessionView::Bytes(shared));
                }
                SessionState::Open(snapshot) => snapshot,
                SessionState::Cold => return Err(()),
            }
        };
        let copied = tokio::task::spawn_blocking(move || snapshot.backup_bytes())
            .await
            .map_err(|_| ())?;
        match copied {
            Ok(SnapshotCopy::Ready(bytes)) => {
                let bytes = Arc::new(bytes);
                *self.state.lock().map_err(|_| ())? = SessionState::Bytes(Arc::clone(&bytes));
                Ok(SessionView::Bytes(bytes))
            }
            Ok(SnapshotCopy::Deferred(streams)) => {
                *self.state.lock().map_err(|_| ())? = SessionState::Deferred(streams.clone());
                Ok(SessionView::Deferred(streams))
            }
            Err(()) => {
                *self.state.lock().map_err(|_| ())? = SessionState::Unreadable;
                Ok(SessionView::Unreadable)
            }
        }
    }
}

struct CredentialCache {
    path: PathBuf,
    slot: Mutex<Option<CredState>>,
}

#[derive(Clone)]
enum CredState {
    Absent,
    Read(CredentialRead),
    Unreadable,
}

impl CredentialCache {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            slot: Mutex::new(None),
        }
    }

    fn load(&self) -> CredState {
        if let Some(cached) = self.slot.lock().ok().and_then(|slot| slot.clone()) {
            return cached;
        }
        let state = if !self.path.exists() {
            CredState::Absent
        } else {
            match fs::read(&self.path) {
                Ok(bytes) => match read_credentials(&bytes) {
                    Ok(read) => CredState::Read(read),
                    Err(CredentialReadError::NotARegistry | CredentialReadError::Record) => {
                        CredState::Unreadable
                    }
                },
                Err(_) => CredState::Unreadable,
            }
        };
        if let Ok(mut slot) = self.slot.lock() {
            *slot = Some(state.clone());
        }
        state
    }
}

struct CanonicalFile {
    files: Arc<SessionFiles>,
}

#[async_trait]
impl ExportSource for CanonicalFile {
    fn kind(&self) -> StoreKind {
        StoreKind::CanonicalRecords
    }

    async fn pin(&self) -> Result<Box<dyn PinnedStore>, ()> {
        self.files.pin().await?;
        Ok(Box::new(CanonicalPin {
            files: self.files.clone(),
        }))
    }
}

struct CanonicalPin {
    files: Arc<SessionFiles>,
}

#[async_trait]
impl PinnedStore for CanonicalPin {
    async fn read(&mut self) -> Result<SourceCut, ()> {
        match self.files.view().await? {
            SessionView::Absent => absent(StoreKind::CanonicalRecords),
            SessionView::Unreadable => skipped_file(StoreKind::CanonicalRecords, "records.sqlite3"),
            SessionView::Deferred(streams) => {
                finish_deferred(StoreKind::CanonicalRecords, &streams)
            }
            SessionView::Bytes(bytes) => {
                // The file is one record. A private-key header or a secret
                // JSON key inside it cannot be stripped per event, so the
                // whole file is fingerprinted, named as a skip, and the
                // bytes are not published.
                if content_is_secret(bytes.as_slice()) {
                    let mut records = Vec::new();
                    let mut skipped = Vec::new();
                    withhold(
                        StoreKind::CanonicalRecords,
                        "records.sqlite3",
                        (*bytes).clone(),
                        &mut records,
                        &mut skipped,
                    );
                    finish(StoreKind::CanonicalRecords, "file", records, skipped)
                } else {
                    let record = public(
                        StoreKind::CanonicalRecords,
                        "records.sqlite3",
                        (*bytes).clone(),
                    )?;
                    finish(StoreKind::CanonicalRecords, "file", vec![record], vec![])
                }
            }
        }
    }
}

struct NotesFile {
    files: Arc<SessionFiles>,
}

#[async_trait]
impl ExportSource for NotesFile {
    fn kind(&self) -> StoreKind {
        StoreKind::Notes
    }

    async fn pin(&self) -> Result<Box<dyn PinnedStore>, ()> {
        self.files.pin().await?;
        Ok(Box::new(NotesPin {
            files: self.files.clone(),
        }))
    }
}

struct NotesPin {
    files: Arc<SessionFiles>,
}

#[async_trait]
impl PinnedStore for NotesPin {
    async fn read(&mut self) -> Result<SourceCut, ()> {
        // Notes live in the canonical session file. Naming each one waits on
        // an event-stream export. The watermark is that snapshot's fingerprint,
        // so the cut shows they were taken together.
        match self.files.view().await? {
            SessionView::Absent => absent(StoreKind::Notes),
            SessionView::Unreadable => skipped_file(StoreKind::Notes, "records.sqlite3"),
            SessionView::Deferred(streams) => finish_deferred(StoreKind::Notes, &streams),
            SessionView::Bytes(bytes) => {
                let fingerprint = crate::backup::domain::Fingerprint::of(bytes.as_slice());
                finish(StoreKind::Notes, &fingerprint.hex(), vec![], vec![])
            }
        }
    }
}

struct MetadataRows {
    facts: Arc<FactsCache>,
}

#[async_trait]
impl ExportSource for MetadataRows {
    fn kind(&self) -> StoreKind {
        StoreKind::ConversationMetadata
    }

    async fn pin(&self) -> Result<Box<dyn PinnedStore>, ()> {
        self.facts.pin().await?;
        Ok(Box::new(MetadataPin {
            facts: self.facts.clone(),
        }))
    }
}

struct MetadataPin {
    facts: Arc<FactsCache>,
}

#[async_trait]
impl PinnedStore for MetadataPin {
    async fn read(&mut self) -> Result<SourceCut, ()> {
        let Some(facts) = self.facts.load().await? else {
            return absent(StoreKind::ConversationMetadata);
        };
        named(
            StoreKind::ConversationMetadata,
            &facts.watermark,
            facts
                .conversations
                .iter()
                .chain(facts.summaries.iter())
                .chain(facts.tombstones.iter()),
            facts
                .skipped
                .iter()
                .filter(|skip| matches!(skip, ExportSkip::Metadata { .. })),
        )
    }
}

struct GrantRows {
    facts: Arc<FactsCache>,
}

#[async_trait]
impl ExportSource for GrantRows {
    fn kind(&self) -> StoreKind {
        StoreKind::ShareGrants
    }

    async fn pin(&self) -> Result<Box<dyn PinnedStore>, ()> {
        self.facts.pin().await?;
        Ok(Box::new(GrantPin {
            facts: self.facts.clone(),
        }))
    }
}

struct GrantPin {
    facts: Arc<FactsCache>,
}

#[async_trait]
impl PinnedStore for GrantPin {
    async fn read(&mut self) -> Result<SourceCut, ()> {
        let Some(facts) = self.facts.load().await? else {
            return absent(StoreKind::ShareGrants);
        };
        named(
            StoreKind::ShareGrants,
            &facts.watermark,
            facts.grants.iter(),
            facts
                .skipped
                .iter()
                .filter(|skip| matches!(skip, ExportSkip::Grant { .. })),
        )
    }
}

struct DirectoryStore {
    kind: StoreKind,
    roots: Vec<PathBuf>,
    extra_files: Vec<PathBuf>,
}

#[async_trait]
impl ExportSource for DirectoryStore {
    fn kind(&self) -> StoreKind {
        self.kind
    }

    async fn pin(&self) -> Result<Box<dyn PinnedStore>, ()> {
        copied(self.cut())
    }
}

impl DirectoryStore {
    fn cut(&self) -> Result<SourceCut, ()> {
        let mut records = Vec::new();
        let mut skipped = Vec::new();
        let mut present = false;
        for root in &self.roots {
            if !root.exists() {
                continue;
            }
            present = true;
            walk(self.kind, root, root, &mut records, &mut skipped);
        }
        for path in &self.extra_files {
            if !path.exists() {
                continue;
            }
            present = true;
            let identity = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("file");
            push_store_file(self.kind, identity, path, &mut records, &mut skipped);
        }
        if !present {
            return absent(self.kind);
        }
        finish(
            self.kind,
            &records.len().saturating_add(skipped.len()).to_string(),
            records,
            skipped,
        )
    }
}

struct JsonFile {
    kind: StoreKind,
    path: PathBuf,
}

#[async_trait]
impl ExportSource for JsonFile {
    fn kind(&self) -> StoreKind {
        self.kind
    }

    async fn pin(&self) -> Result<Box<dyn PinnedStore>, ()> {
        copied(self.cut())
    }
}

impl JsonFile {
    fn cut(&self) -> Result<SourceCut, ()> {
        if !self.path.exists() {
            return absent(self.kind);
        }
        let mut records = Vec::new();
        let mut skipped = Vec::new();
        push_path(
            self.kind,
            "grants.json",
            &self.path,
            &mut records,
            &mut skipped,
        );
        finish(self.kind, "file", records, skipped)
    }
}

struct JsonlFile {
    kind: StoreKind,
    path: PathBuf,
}

#[async_trait]
impl ExportSource for JsonlFile {
    fn kind(&self) -> StoreKind {
        self.kind
    }

    async fn pin(&self) -> Result<Box<dyn PinnedStore>, ()> {
        copied(self.cut())
    }
}

impl JsonlFile {
    fn cut(&self) -> Result<SourceCut, ()> {
        if !self.path.exists() {
            return absent(self.kind);
        }
        match read_public_text(&self.path) {
            TextRead::Missing => Err(()),
            TextRead::Secret(bytes) => {
                let identity = file_name(&self.path);
                let mut records = Vec::new();
                let mut skipped = Vec::new();
                withhold(self.kind, &identity, bytes, &mut records, &mut skipped);
                finish(self.kind, "secret", records, skipped)
            }
            TextRead::Text(text) => jsonl_cut(self.kind, &text).map_err(|_| ()),
        }
    }
}

struct RelayStore {
    namespace: PathBuf,
}

#[async_trait]
impl ExportSource for RelayStore {
    fn kind(&self) -> StoreKind {
        StoreKind::RelayConfiguration
    }

    async fn pin(&self) -> Result<Box<dyn PinnedStore>, ()> {
        copied(self.cut())
    }
}

impl RelayStore {
    fn cut(&self) -> Result<SourceCut, ()> {
        let mut records = Vec::new();
        let mut skipped = Vec::new();
        let config = self.namespace.join("config.json");
        if config.exists() {
            match fs::read(&config) {
                Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                    Ok(value) => {
                        if let Some(relay) =
                            value.get("native").and_then(|native| native.get("relay"))
                        {
                            if !relay.is_null() {
                                let encoded = serde_json::to_vec(relay).map_err(|_| ())?;
                                records.push(public(
                                    StoreKind::RelayConfiguration,
                                    "gateway",
                                    encoded,
                                )?);
                            }
                        }
                    }
                    Err(_) => skipped.push(skip(StoreKind::RelayConfiguration, "config.json")?),
                },
                Err(_) => skipped.push(skip(StoreKind::RelayConfiguration, "config.json")?),
            }
        }
        let peers = self.namespace.join("peer-gateways");
        if peers.is_dir() {
            let entries = fs::read_dir(&peers).map_err(|_| ())?;
            for entry in entries {
                let entry = entry.map_err(|_| ())?;
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                let Ok(bytes) = fs::read(&path) else {
                    skipped.push(skip(
                        StoreKind::RelayConfiguration,
                        &file_identity(&peers, &path),
                    )?);
                    continue;
                };
                let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                    continue;
                };
                if let Some(relay) = value.get("relay").filter(|relay| !relay.is_null()) {
                    let encoded = serde_json::to_vec(relay).map_err(|_| ())?;
                    let identity = format!("peer-{}", file_identity(&peers, &path));
                    records.push(public(StoreKind::RelayConfiguration, &identity, encoded)?);
                }
            }
        }
        if records.is_empty() && skipped.is_empty() && !config.exists() && !peers.exists() {
            return absent(StoreKind::RelayConfiguration);
        }
        finish(
            StoreKind::RelayConfiguration,
            &records.len().saturating_add(skipped.len()).to_string(),
            records,
            skipped,
        )
    }
}

struct CredentialStore {
    kind: StoreKind,
    credentials: Arc<CredentialCache>,
    /// `None` is the worker store. The registry has no separate worker class.
    wanted: Option<CredentialKind>,
}

#[async_trait]
impl ExportSource for CredentialStore {
    fn kind(&self) -> StoreKind {
        self.kind
    }

    async fn pin(&self) -> Result<Box<dyn PinnedStore>, ()> {
        // The registry file is small. Copying it here, while admission is
        // held, is what a later read of this pin returns.
        copied(self.cut())
    }
}

impl CredentialStore {
    fn cut(&self) -> Result<SourceCut, ()> {
        // The registry does not store a worker apart from an environment
        // peer. This store stays empty unless that changes; an unreadable
        // registry is still reported.
        let Some(wanted) = self.wanted else {
            return match self.credentials.load() {
                CredState::Unreadable => Err(()),
                CredState::Absent => absent(self.kind),
                CredState::Read(_) => finish(self.kind, "0", vec![], vec![]),
            };
        };
        match self.credentials.load() {
            CredState::Absent => absent(self.kind),
            CredState::Unreadable => Err(()),
            CredState::Read(read) => {
                let mut records = Vec::new();
                let mut skipped = Vec::new();
                if wanted == CredentialKind::Device {
                    for identity in &read.skipped {
                        skipped.push(skip(self.kind, identity)?);
                    }
                }
                for credential in read
                    .credentials
                    .iter()
                    .filter(|credential| credential.kind == wanted)
                {
                    records.push(public(
                        self.kind,
                        &credential.id,
                        credential.record.clone(),
                    )?);
                }
                finish(self.kind, &records.len().to_string(), records, skipped)
            }
        }
    }
}

struct Receivers {
    path: PathBuf,
    state: Arc<Mutex<ReceiverState>>,
}

enum ReceiverState {
    Cold,
    Absent,
    Open(ReceiverSnapshot),
    Failed,
}

impl Receivers {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            state: Arc::new(Mutex::new(ReceiverState::Cold)),
        }
    }
}

#[async_trait]
impl ExportSource for Receivers {
    fn kind(&self) -> StoreKind {
        StoreKind::ReceiverAccess
    }

    async fn pin(&self) -> Result<Box<dyn PinnedStore>, ()> {
        {
            let guard = self.state.lock().map_err(|_| ())?;
            if !matches!(*guard, ReceiverState::Cold) {
                return Ok(Box::new(ReceiverPin {
                    state: self.state.clone(),
                }));
            }
        }
        if !self.path.is_file() {
            *self.state.lock().map_err(|_| ())? = ReceiverState::Absent;
        } else {
            let path = self.path.clone();
            let opened = tokio::task::spawn_blocking(move || ReceiverSnapshot::open(&path))
                .await
                .map_err(|_| ())?;
            let mut guard = self.state.lock().map_err(|_| ())?;
            if matches!(*guard, ReceiverState::Cold) {
                *guard = match opened {
                    Ok(snapshot) => ReceiverState::Open(snapshot),
                    Err(ReceiverSnapshotError::Unavailable) => ReceiverState::Failed,
                };
            }
        }
        Ok(Box::new(ReceiverPin {
            state: self.state.clone(),
        }))
    }
}

struct ReceiverPin {
    state: Arc<Mutex<ReceiverState>>,
}

#[async_trait]
impl PinnedStore for ReceiverPin {
    async fn read(&mut self) -> Result<SourceCut, ()> {
        let snapshot = {
            let mut guard = self.state.lock().map_err(|_| ())?;
            match std::mem::replace(&mut *guard, ReceiverState::Cold) {
                ReceiverState::Absent => {
                    *guard = ReceiverState::Absent;
                    return absent(StoreKind::ReceiverAccess);
                }
                ReceiverState::Failed => {
                    *guard = ReceiverState::Failed;
                    return Err(());
                }
                ReceiverState::Open(snapshot) => snapshot,
                ReceiverState::Cold => return Err(()),
            }
        };
        let read = tokio::task::spawn_blocking(move || snapshot.read())
            .await
            .map_err(|_| ())?;
        let (watermark, rows, skipped_ids) = read.map_err(|_| ())?;
        let mut records = Vec::new();
        let mut skipped = Vec::new();
        for row in rows {
            let bytes = serde_json::to_vec(&serde_json::json!({
                "receiverId": row.receiver_id,
                "credentialId": row.credential_id,
                "accessEpoch": row.access_epoch,
                "active": row.active,
            }))
            .map_err(|_| ())?;
            records.push(public(StoreKind::ReceiverAccess, &row.receiver_id, bytes)?);
        }
        for identity in skipped_ids {
            skipped.push(skip(StoreKind::ReceiverAccess, &identity)?);
        }
        finish(StoreKind::ReceiverAccess, &watermark, records, skipped)
    }
}

struct InventoryCut<'a> {
    log: &'a dyn DeletionLogStore,
}

#[async_trait]
impl ExportSource for InventoryCut<'_> {
    fn kind(&self) -> StoreKind {
        StoreKind::DeletionInventory
    }

    async fn pin(&self) -> Result<Box<dyn PinnedStore>, ()> {
        // The log is small, and this pin runs after the fence append, still
        // inside the admission hold. The bytes below are that log.
        copied(self.cut().await)
    }
}

impl InventoryCut<'_> {
    async fn cut(&self) -> Result<SourceCut, ()> {
        let log = self.log.load().await?;
        let bytes = log.canonical().into_bytes();
        let record = public(StoreKind::DeletionInventory, "log", bytes)?;
        finish(
            StoreKind::DeletionInventory,
            &log.watermark(),
            vec![record],
            vec![],
        )
    }
}

struct NamespaceFences {
    facts: Arc<FactsCache>,
    credentials: Arc<CredentialCache>,
}

#[async_trait]
impl FenceSource for NamespaceFences {
    async fn fences(&self) -> Result<Vec<DeletionFence>, ()> {
        let mut fences = Vec::new();
        let mut failed = false;
        match self.facts.fence_facts().await {
            Ok(Some(facts)) => {
                fences.extend(fences_from_facts(&facts));
            }
            Ok(None) => {}
            Err(()) => failed = true,
        }
        match self.credentials.load() {
            CredState::Read(read) => {
                for credential in read
                    .credentials
                    .iter()
                    .filter(|credential| credential.revoked)
                {
                    if let Ok(fence) = DeletionFence::credential(
                        credential.kind,
                        &credential.id,
                        credential.fingerprint,
                    ) {
                        fences.push(fence);
                    }
                }
            }
            CredState::Absent => {}
            CredState::Unreadable => failed = true,
        }
        if failed && fences.is_empty() {
            return Err(());
        }
        Ok(fences)
    }
}

fn fences_from_facts(facts: &ExportFences) -> Vec<DeletionFence> {
    let mut fences = Vec::new();
    for tombstone in &facts.tombstones {
        let Ok(value) = serde_json::from_slice::<Value>(&tombstone.bytes) else {
            continue;
        };
        let Some(id) = value.get("conversationId").and_then(Value::as_str) else {
            continue;
        };
        if let Ok(fence) = DeletionFence::conversation(id) {
            fences.push(fence);
        }
    }
    for revocation in &facts.revocations {
        let Some(role) = GrantRole::parse(&revocation.role) else {
            continue;
        };
        if let Ok(fence) = DeletionFence::share_grant(
            &revocation.conversation_id,
            &revocation.receiver_id,
            &revocation.credential_id,
            role,
            revocation.sequence,
        ) {
            fences.push(fence);
        }
    }
    fences
}

fn named<'a>(
    kind: StoreKind,
    watermark: &str,
    records: impl Iterator<Item = &'a NamedRecord>,
    skipped: impl Iterator<Item = &'a ExportSkip>,
) -> Result<SourceCut, ()> {
    let mut captured = Vec::new();
    for record in records {
        captured.push(public(kind, &record.identity, record.bytes.clone())?);
    }
    let mut skips = Vec::new();
    for skip_row in skipped {
        let identity = match skip_row {
            ExportSkip::Metadata { identity } | ExportSkip::Grant { identity } => identity,
        };
        skips.push(skip(kind, identity)?);
    }
    finish(kind, watermark, captured, skips)
}

fn walk(
    kind: StoreKind,
    root: &Path,
    directory: &Path,
    records: &mut Vec<CapturedRecord>,
    skipped: &mut Vec<SkippedRecord>,
) {
    let Ok(entries) = fs::read_dir(directory) else {
        if let Ok(record) = skip(kind, &file_identity(root, directory)) {
            skipped.push(record);
        }
        return;
    };
    let mut index = 0u64;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                index += 1;
                if let Ok(record) = skip(kind, &format!("entry-{index}")) {
                    skipped.push(record);
                }
                continue;
            }
        };
        let path = entry.path();
        if path
            .file_name()
            .is_some_and(|name| name == "outbox" || name == "inbox")
        {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            if let Ok(record) = skip(kind, &file_identity(root, &path)) {
                skipped.push(record);
            }
            continue;
        };
        if metadata.file_type().is_symlink() {
            if let Ok(record) = skip(kind, &file_identity(root, &path)) {
                skipped.push(record);
            }
            continue;
        }
        if metadata.is_dir() {
            walk(kind, root, &path, records, skipped);
            continue;
        }
        push_path(kind, &file_identity(root, &path), &path, records, skipped);
    }
}

fn push_store_file(
    kind: StoreKind,
    identity: &str,
    path: &Path,
    records: &mut Vec<CapturedRecord>,
    skipped: &mut Vec<SkippedRecord>,
) {
    if path_sensitivity(path) == Sensitivity::Secret
        || path.extension().and_then(|ext| ext.to_str()) != Some("jsonl")
    {
        push_path(kind, identity, path, records, skipped);
    } else {
        push_jsonl(kind, path, records, skipped);
    }
}

fn push_jsonl(
    kind: StoreKind,
    path: &Path,
    records: &mut Vec<CapturedRecord>,
    skipped: &mut Vec<SkippedRecord>,
) {
    let text = match read_public_text(path) {
        TextRead::Missing => {
            if let Ok(record) = skip(kind, "jsonl") {
                skipped.push(record);
            }
            return;
        }
        TextRead::Secret(bytes) => {
            withhold(kind, &file_name(path), bytes, records, skipped);
            return;
        }
        TextRead::Text(text) => text,
    };
    match jsonl_cut(kind, &text) {
        Ok(cut) => {
            records.extend(cut.records().iter().cloned());
            skipped.extend(cut.skipped().iter().cloned());
        }
        Err(_) => {
            if let Ok(record) = skip(kind, "jsonl") {
                skipped.push(record);
            }
        }
    }
}

enum TextRead {
    Missing,
    Secret(Vec<u8>),
    Text(String),
}

fn read_public_text(path: &Path) -> TextRead {
    let Ok(bytes) = fs::read(path) else {
        return TextRead::Missing;
    };
    if content_is_secret(&bytes) {
        return TextRead::Secret(bytes);
    }
    match String::from_utf8(bytes) {
        Ok(text) => TextRead::Text(text),
        Err(_) => TextRead::Missing,
    }
}

fn push_path(
    kind: StoreKind,
    identity: &str,
    path: &Path,
    records: &mut Vec<CapturedRecord>,
    skipped: &mut Vec<SkippedRecord>,
) {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => {
            if let Ok(record) = skip(kind, identity) {
                skipped.push(record);
            }
            return;
        }
    };
    let content_secret = content_is_secret(&bytes);
    let secret = path_sensitivity(path) == Sensitivity::Secret || content_secret;
    if secret && content_secret {
        withhold(kind, identity, bytes, records, skipped);
        return;
    }
    let captured = if secret {
        CapturedRecord::new(kind, identity, Sensitivity::Secret, bytes)
    } else {
        CapturedRecord::new(kind, identity, Sensitivity::Public, bytes)
    };
    match captured {
        Ok(record) => records.push(record),
        Err(_) => {
            if let Ok(record) = skip(kind, "unnameable") {
                skipped.push(record);
            }
        }
    }
}

/// JSON keys whose values are key material. A quoted `"token"` or
/// `"secret"` in ordinary conversation text is not one of these.
const SECRET_JSON_KEYS: &[&str] = &["verifier", "privateKey", "private_key"];

const PEM_BEGIN: &[u8] = b"-----BEGIN";
const PRIVATE_KEY_LABEL: &[u8] = b"PRIVATE KEY";
/// PEM labels are short. The bound keeps a `-----BEGIN` inside a binary
/// page from being treated as a line that runs to the end of the file.
const PEM_HEADER_LIMIT: usize = 256;

fn content_is_secret(bytes: &[u8]) -> bool {
    bytes_hold_secret(bytes) || json_has_secret(bytes)
}

fn json_has_secret(bytes: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return false;
    };
    secret_key(&value)
}

/// A raw store file, including a session database, that holds key material.
/// The markers are the same keys and private-key headers [`json_has_secret`]
/// refuses in a JSON value.
fn bytes_hold_secret(bytes: &[u8]) -> bool {
    holds_private_key_header(bytes)
        || SECRET_JSON_KEYS.iter().any(|key| {
            let quoted = format!("\"{key}\"");
            find_bytes(bytes, quoted.as_bytes()).is_some()
        })
}

/// A PEM header: `-----BEGIN` through the end of that line, when the line
/// contains `PRIVATE KEY`. RSA, EC, encrypted, OpenSSH, and PKCS#8 headers
/// match. A public key or a certificate does not.
fn holds_private_key_header(bytes: &[u8]) -> bool {
    let mut rest = bytes;
    while let Some(at) = find_bytes(rest, PEM_BEGIN) {
        let after = at + PEM_BEGIN.len();
        let line = &rest[after..];
        let end = line
            .iter()
            .position(|byte| *byte == b'\n' || *byte == b'\r')
            .unwrap_or(line.len())
            .min(PEM_HEADER_LIMIT);
        if find_bytes(&line[..end], PRIVATE_KEY_LABEL).is_some() {
            return true;
        }
        rest = &rest[after..];
    }
    false
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn secret_key(value: &Value) -> bool {
    match value {
        Value::Object(map) => map
            .iter()
            .any(|(key, child)| SECRET_JSON_KEYS.contains(&key.as_str()) || secret_key(child)),
        Value::Array(items) => items.iter().any(secret_key),
        Value::String(text) => holds_private_key_header(text.as_bytes()),
        _ => false,
    }
}

fn withhold(
    kind: StoreKind,
    identity: &str,
    bytes: Vec<u8>,
    records: &mut Vec<CapturedRecord>,
    skipped: &mut Vec<SkippedRecord>,
) {
    match CapturedRecord::new(kind, identity, Sensitivity::Secret, bytes) {
        Ok(record) => records.push(record),
        Err(_) => {
            if let Ok(record) = skip(kind, "unnameable") {
                skipped.push(record);
            }
            return;
        }
    }
    if let Ok(record) = SkippedRecord::new(kind, identity, SkipReason::Secret) {
        skipped.push(record);
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file")
        .to_owned()
}

fn file_identity(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn copied(cut: Result<SourceCut, ()>) -> Result<Box<dyn PinnedStore>, ()> {
    Ok(Box::new(CopiedStore::new(cut?)))
}

fn public(kind: StoreKind, identity: &str, bytes: Vec<u8>) -> Result<CapturedRecord, ()> {
    CapturedRecord::new(kind, identity, Sensitivity::Public, bytes).map_err(|_| ())
}

fn skip(kind: StoreKind, identity: &str) -> Result<SkippedRecord, ()> {
    SkippedRecord::new(kind, identity, SkipReason::Unreadable).map_err(|_| ())
}

fn skipped_file(kind: StoreKind, identity: &str) -> Result<SourceCut, ()> {
    finish(kind, "unavailable", vec![], vec![skip(kind, identity)?])
}

fn finish_deferred(kind: StoreKind, streams: &[DeferredStream]) -> Result<SourceCut, ()> {
    let mut deferred = Vec::with_capacity(streams.len());
    for stream in streams {
        deferred.push(
            DeferredEntry::new(
                kind,
                stream.conversation_id.clone(),
                stream.size,
                Fingerprint::from_digest(stream.digest),
            )
            .map_err(|_| ())?,
        );
    }
    let watermark = crate::backup::domain::Watermark::new("deferred").map_err(|_| ())?;
    SourceCut::with_deferred(kind, watermark, vec![], vec![], deferred).map_err(|_| ())
}

fn absent(kind: StoreKind) -> Result<SourceCut, ()> {
    finish(kind, "absent", vec![], vec![])
}

fn finish(
    kind: StoreKind,
    watermark: &str,
    records: Vec<CapturedRecord>,
    skipped: Vec<SkippedRecord>,
) -> Result<SourceCut, ()> {
    let watermark = crate::backup::domain::Watermark::new(watermark).map_err(|_| ())?;
    SourceCut::new(kind, watermark, records, skipped).map_err(|_| ())
}
