//! Read one namespace into the writer inventory.
//!
//! Admission is held only while each store is pinned: a SQLite read snapshot
//! for a database, a copy of a small file, or an open descriptor and its
//! length for a directory, JSONL, or relay file. Bodies of those descriptors
//! are read after admission is released, and only up to the length recorded
//! at the pin. The device outbox and the environment outbox are read during
//! that pin. Secret paths are fingerprinted.
//! The credential registry contributes public facts only.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use nessa_local_storage::{create_directory, open, OpenMode};

use async_trait::async_trait;
use serde_json::Value;

#[cfg(test)]
#[path = "../../../tests/backup/publication_lock.rs"]
mod publication_lock_tests;

use super::credentials::{read_credentials, CredentialRead, CredentialReadError};
use super::jsonl::jsonl_cut;
use super::sensitivity::path_sensitivity;
use super::snapshot::{
    DeferredStream, SnapshotCopy, SqliteSnapshot, PUBLIC_FILE_LIMIT, SESSION_EXPORT_LIMIT,
};
use crate::backup::application::{
    capture_export, Authenticator, CopiedStore, DeletionLogStore, ExportClock, ExportFailure,
    ExportRequest, ExportSource, FenceSource, ManifestPublisher, MutationAdmission, PinnedStore,
};
use crate::backup::domain::{
    CapturedRecord, CredentialKind, DeferredEntry, DeletionFence, Fingerprint, GrantRole,
    Sensitivity, SkipReason, SkippedRecord, SourceCut, StoreKind,
};
use crate::conversation::infrastructure::{
    BoatRuntimeConfig, ExportFences, ExportSkip, MetadataSnapshot, NamedRecord, ReceiverSnapshot,
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
        namespace.join("receiver-access").join("publication.lock"),
        namespace
            .join("receiver-access")
            .join("receiver-access.sqlite3"),
    ));
    let environment_grants = Arc::new(GrantFiles::new(namespace.join("environment")));
    let inventory_cut = InventoryCut { log: inventory };
    let fences = NamespaceFences {
        namespace: namespace.to_path_buf(),
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
            roots: vec![plain_root(
                namespace
                    .join("conversations")
                    .join("audit")
                    .join("drive-tools"),
            )],
            extra_files: vec![],
            grants: None,
        }),
        Box::new(DirectoryStore {
            kind: StoreKind::EnvironmentAudit,
            roots: vec![
                labeled_root(namespace.join("boat-workspaces"), "boat-workspaces"),
                labeled_root(namespace.join("boat-runtime-audit"), "boat-runtime"),
                labeled_root(
                    namespace.join("boat-environment-audit"),
                    "boat-environments",
                ),
                labeled_root(
                    namespace
                        .join("conversations")
                        .join("audit")
                        .join("warm-up"),
                    "warm-up",
                ),
                labeled_root(
                    namespace
                        .join("conversations")
                        .join("audit")
                        .join("environments"),
                    "environments",
                ),
            ],
            extra_files: vec![],
            grants: Some(environment_grants.clone()),
        }),
        Box::new(DirectoryStore {
            kind: StoreKind::ConversationAudit,
            roots: vec![
                labeled_root(
                    namespace
                        .join("conversations")
                        .join("audit")
                        .join("creation"),
                    "creation",
                ),
                labeled_root(
                    namespace
                        .join("conversations")
                        .join("audit")
                        .join("deletion"),
                    "deletion",
                ),
                labeled_root(
                    namespace
                        .join("conversations")
                        .join("audit")
                        .join("approval-mode"),
                    "approval-mode",
                ),
                labeled_root(
                    namespace
                        .join("conversations")
                        .join("audit")
                        .join("file-links"),
                    "file-links",
                ),
                labeled_root(
                    namespace
                        .join("conversations")
                        .join("audit")
                        .join("mcp-apps"),
                    "mcp-apps",
                ),
                labeled_root(
                    namespace
                        .join("conversations")
                        .join("audit")
                        .join("execution"),
                    "execution",
                ),
            ],
            extra_files: vec![],
            grants: None,
        }),
        Box::new(DirectoryStore {
            kind: StoreKind::PeerPairings,
            roots: vec![
                labeled_root(namespace.join("peer-gateways"), "peer-gateways"),
                labeled_root(namespace.join("peer-gateways-audit"), "peer-gateways-audit"),
            ],
            extra_files: vec![],
            grants: None,
        }),
        Box::new(DirectoryStore {
            kind: StoreKind::EnvironmentPeerPairings,
            roots: vec![plain_root(namespace.join("environment").join("pairing"))],
            extra_files: vec![namespace.join("environment").join("paired-with")],
            grants: None,
        }),
        Box::new(DirectoryStore {
            kind: StoreKind::WorkerEnrollments,
            roots: vec![
                labeled_root(namespace.join("workers"), "workers"),
                labeled_root(
                    namespace.join("environment").join("workers"),
                    "environment-workers",
                ),
            ],
            extra_files: vec![],
            grants: None,
        }),
        Box::new(EnvironmentGrantFile {
            grants: environment_grants,
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
            roots: vec![plain_root(namespace.join("attachments").join("holds"))],
            extra_files: vec![],
            grants: None,
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
        Box::new(Receivers::new(credentials.clone())),
        // Both pins take `outbox.lock` shared for the whole read. The
        // environment writer holds that file exclusively across stage,
        // discard, close, and the sweep when serving starts. Each
        // directory stops at the files and bytes one lease may keep.
        Box::new(super::outbox::OutboxDirectory::new(
            StoreKind::DeviceOutbox,
            namespace.join("device").join("outbox"),
        )),
        Box::new(super::outbox::OutboxDirectory::new(
            StoreKind::EnvironmentOutbox,
            namespace.join("environment").join("outbox"),
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
        // A row the fence read skipped fails this source. The snapshot stays
        // open so the metadata and grant cuts can still name that row.
        *self.state.lock().map_err(|_| ())? = FactsState::Open(snapshot);
        match fences {
            Ok(fences) => Ok(Some(fences)),
            Err(_) => Err(()),
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
            Err(()) => match read_capped(&path, SESSION_EXPORT_LIMIT) {
                CappedFile::Bytes(bytes) => SessionState::Bytes(Arc::new(bytes)),
                CappedFile::Missing | CappedFile::Unreadable | CappedFile::Bounded => {
                    SessionState::Unreadable
                }
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
    lock_path: PathBuf,
    receiver_path: PathBuf,
    slot: Mutex<Option<CredState>>,
    receivers: Arc<Mutex<ReceiverState>>,
}

#[derive(Clone)]
enum CredState {
    Absent,
    Read(CredentialRead),
    Unreadable,
}

impl CredentialCache {
    fn new(path: PathBuf, lock_path: PathBuf, receiver_path: PathBuf) -> Self {
        Self {
            path,
            lock_path,
            receiver_path,
            slot: Mutex::new(None),
            receivers: Arc::new(Mutex::new(ReceiverState::Cold)),
        }
    }

    /// Read the registry and open the receiver snapshot while
    /// `publication.lock` is held. Its parent is created if absent so the
    /// first pairing writer uses the same lock. A later call returns the same pair.
    fn load(&self) -> CredState {
        if let Some(cached) = self.slot.lock().ok().and_then(|slot| slot.clone()) {
            return cached;
        }
        let path = self.path.clone();
        let lock_path = self.lock_path.clone();
        let receiver_path = self.receiver_path.clone();
        let (credentials, receivers) = snapshot_paired(&lock_path, &path, &receiver_path);
        if let Ok(mut slot) = self.receivers.lock() {
            if matches!(*slot, ReceiverState::Cold) {
                *slot = receivers;
            }
        }
        if let Ok(mut slot) = self.slot.lock() {
            if slot.is_none() {
                *slot = Some(credentials.clone());
            }
            return slot.clone().unwrap_or(credentials);
        }
        credentials
    }
}

fn snapshot_paired(
    lock_path: &Path,
    credentials: &Path,
    receivers: &Path,
) -> (CredState, ReceiverState) {
    // Establish the lock before either store is inspected, including first use.
    // A lock error makes both stores unreadable.
    let lock = match publication_lock(lock_path) {
        Ok(file) => file,
        Err(()) => return (CredState::Unreadable, ReceiverState::Failed),
    };
    let credentials = read_credential_state(credentials);
    let receivers = open_receiver_state(receivers);
    drop(lock);
    (credentials, receivers)
}

fn publication_lock(path: &Path) -> Result<File, ()> {
    let Some(parent) = path.parent() else {
        return Err(());
    };
    match fs::symlink_metadata(parent) {
        Ok(meta) if meta.is_dir() => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {
            create_directory(parent).map_err(|_| ())?;
        }
        _ => return Err(()),
    }
    let file = open(path, OpenMode::OpenOrCreate).map_err(|_| ())?;
    file.lock().map_err(|_| ())?;
    Ok(file)
}

fn read_credential_state(path: &Path) -> CredState {
    if !path.exists() {
        return CredState::Absent;
    }
    match fs::read(path) {
        Ok(bytes) => match read_credentials(&bytes) {
            Ok(read) => CredState::Read(read),
            Err(CredentialReadError::NotARegistry | CredentialReadError::Record) => {
                CredState::Unreadable
            }
        },
        Err(_) => CredState::Unreadable,
    }
}

fn open_receiver_state(path: &Path) -> ReceiverState {
    if !path.is_file() {
        return ReceiverState::Absent;
    }
    match ReceiverSnapshot::open(path) {
        Ok(snapshot) => ReceiverState::Open(snapshot),
        Err(ReceiverSnapshotError::Unavailable) => ReceiverState::Failed,
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

#[derive(Clone)]
struct LabeledRoot {
    path: PathBuf,
    /// Prefixed onto each identity so two roots cannot share one name.
    label: &'static str,
}

fn plain_root(path: PathBuf) -> LabeledRoot {
    LabeledRoot { path, label: "" }
}

fn labeled_root(path: PathBuf, label: &'static str) -> LabeledRoot {
    LabeledRoot { path, label }
}

struct DirectoryStore {
    kind: StoreKind,
    roots: Vec<LabeledRoot>,
    extra_files: Vec<PathBuf>,
    /// Set for the environment audit, which also keeps `grants-audit.jsonl`.
    /// That journal is snapshotted with `grants.json` under `grants.lock`.
    grants: Option<Arc<GrantFiles>>,
}

#[async_trait]
impl ExportSource for DirectoryStore {
    fn kind(&self) -> StoreKind {
        self.kind
    }

    async fn pin(&self) -> Result<Box<dyn PinnedStore>, ()> {
        // `grants-audit.jsonl` is a small file copied under its writer lock.
        // The other files are opened here and read after the hold.
        if let Some(grants) = &self.grants {
            grants.pin().await?;
        }
        let roots = self.roots.clone();
        let extra_files = self.extra_files.clone();
        let frozen = tokio::task::spawn_blocking(move || freeze_directory(&roots, &extra_files))
            .await
            .map_err(|_| ())?;
        Ok(Box::new(DirectoryPin {
            kind: self.kind,
            present: frozen.present,
            files: frozen.files,
            grants: self.grants.clone(),
        }))
    }
}

struct DirectoryPin {
    kind: StoreKind,
    present: bool,
    files: Vec<FrozenFile>,
    grants: Option<Arc<GrantFiles>>,
}

#[async_trait]
impl PinnedStore for DirectoryPin {
    async fn read(&mut self) -> Result<SourceCut, ()> {
        let kind = self.kind;
        let present = self.present;
        let files = std::mem::take(&mut self.files);
        let grants = self.grants.clone();
        tokio::task::spawn_blocking(move || assemble_directory(kind, present, files, grants))
            .await
            .map_err(|_| ())?
    }
}

fn assemble_directory(
    kind: StoreKind,
    present: bool,
    files: Vec<FrozenFile>,
    grants: Option<Arc<GrantFiles>>,
) -> Result<SourceCut, ()> {
    let mut records = Vec::new();
    let mut skipped = Vec::new();
    materialize(kind, files, &mut records, &mut skipped);
    if let Some(grants) = grants {
        let snapshot = grants.view()?;
        push_capped_jsonl(
            kind,
            "grants-audit.jsonl",
            snapshot.audit,
            &mut records,
            &mut skipped,
        )?;
    }
    if !present && records.is_empty() && skipped.is_empty() {
        return absent(kind);
    }
    finish(
        kind,
        &records.len().saturating_add(skipped.len()).to_string(),
        records,
        skipped,
    )
}

/// `grants.json` and `grants-audit.jsonl`, copied together under `grants.lock`.
struct GrantFiles {
    directory: PathBuf,
    state: Mutex<Option<GrantSnapshot>>,
}

#[derive(Clone)]
struct GrantSnapshot {
    grants: CappedFile,
    audit: CappedFile,
}

impl GrantFiles {
    fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            state: Mutex::new(None),
        }
    }

    /// Copy both files while holding the environment grant lock. A second pin
    /// returns the same copy, so the audit store and the grant store publish
    /// one cut.
    async fn pin(&self) -> Result<(), ()> {
        if self.state.lock().map_err(|_| ())?.is_some() {
            return Ok(());
        }
        let directory = self.directory.clone();
        let snapshot = tokio::task::spawn_blocking(move || snapshot_grants(&directory))
            .await
            .map_err(|_| ())?;
        let mut guard = self.state.lock().map_err(|_| ())?;
        if guard.is_none() {
            *guard = Some(snapshot);
        }
        Ok(())
    }

    fn view(&self) -> Result<GrantSnapshot, ()> {
        self.state.lock().map_err(|_| ())?.clone().ok_or(())
    }
}

fn snapshot_grants(directory: &Path) -> GrantSnapshot {
    if !directory.is_dir() {
        return GrantSnapshot {
            grants: CappedFile::Missing,
            audit: CappedFile::Missing,
        };
    }
    let locked = (|| {
        let file = open(&directory.join("grants.lock"), OpenMode::OpenOrCreate)?;
        file.lock()?;
        Ok::<_, std::io::Error>(file)
    })();
    let Ok(lock) = locked else {
        return GrantSnapshot {
            grants: CappedFile::Unreadable,
            audit: CappedFile::Unreadable,
        };
    };
    let grants = read_capped(&directory.join("grants.json"), PUBLIC_FILE_LIMIT);
    let audit = read_capped(&directory.join("grants-audit.jsonl"), PUBLIC_FILE_LIMIT);
    drop(lock);
    GrantSnapshot { grants, audit }
}

struct EnvironmentGrantFile {
    grants: Arc<GrantFiles>,
}

#[async_trait]
impl ExportSource for EnvironmentGrantFile {
    fn kind(&self) -> StoreKind {
        StoreKind::EnvironmentGrants
    }

    async fn pin(&self) -> Result<Box<dyn PinnedStore>, ()> {
        self.grants.pin().await?;
        Ok(Box::new(EnvironmentGrantPin {
            grants: self.grants.clone(),
        }))
    }
}

struct EnvironmentGrantPin {
    grants: Arc<GrantFiles>,
}

#[async_trait]
impl PinnedStore for EnvironmentGrantPin {
    async fn read(&mut self) -> Result<SourceCut, ()> {
        let snapshot = self.grants.view()?;
        if matches!(snapshot.grants, CappedFile::Missing) {
            return absent(StoreKind::EnvironmentGrants);
        }
        let mut records = Vec::new();
        let mut skipped = Vec::new();
        push_capped_record(
            StoreKind::EnvironmentGrants,
            "grants.json",
            snapshot.grants,
            &mut records,
            &mut skipped,
        )?;
        finish(StoreKind::EnvironmentGrants, "file", records, skipped)
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
        let kind = self.kind;
        let path = self.path.clone();
        let pinned = tokio::task::spawn_blocking(move || pin_jsonl(kind, &path))
            .await
            .map_err(|_| ())?;
        Ok(Box::new(JsonlPin { pinned }))
    }
}

enum PinnedJsonl {
    Absent(StoreKind),
    File(StoreKind, FrozenFile),
}

struct JsonlPin {
    pinned: PinnedJsonl,
}

#[async_trait]
impl PinnedStore for JsonlPin {
    async fn read(&mut self) -> Result<SourceCut, ()> {
        let pinned = std::mem::replace(
            &mut self.pinned,
            PinnedJsonl::Absent(StoreKind::LeaseRecords),
        );
        tokio::task::spawn_blocking(move || read_jsonl(pinned))
            .await
            .map_err(|_| ())?
    }
}

fn pin_jsonl(kind: StoreKind, path: &Path) -> PinnedJsonl {
    if !path.exists() {
        return PinnedJsonl::Absent(kind);
    }
    PinnedJsonl::File(kind, freeze_one(file_name(path), path, true))
}

fn read_jsonl(pinned: PinnedJsonl) -> Result<SourceCut, ()> {
    let (kind, file) = match pinned {
        PinnedJsonl::Absent(kind) => return absent(kind),
        PinnedJsonl::File(kind, file) => (kind, file),
    };
    let mut records = Vec::new();
    let mut skipped = Vec::new();
    match read_frozen(&file)? {
        FrozenRead::Unreadable => Err(()),
        FrozenRead::Bounded => finish(
            kind,
            "bounded",
            vec![],
            vec![SkippedRecord::new(kind, &file.identity, SkipReason::Bounded).map_err(|_| ())?],
        ),
        FrozenRead::Bytes(bytes) => {
            if content_is_secret(&bytes) {
                withhold(kind, &file.identity, bytes, &mut records, &mut skipped);
                finish(kind, "secret", records, skipped)
            } else {
                let Ok(text) = String::from_utf8(bytes) else {
                    return Err(());
                };
                jsonl_cut(kind, &text).map_err(|_| ())
            }
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
        let namespace = self.namespace.clone();
        let pinned = tokio::task::spawn_blocking(move || pin_relay(&namespace))
            .await
            .map_err(|_| ())?;
        Ok(Box::new(RelayPin { pinned }))
    }
}

struct RelayPin {
    pinned: PinnedRelay,
}

struct PinnedRelay {
    config: Option<FrozenFile>,
    peers: Vec<FrozenFile>,
    config_exists: bool,
    peers_exist: bool,
    peers_unreadable: bool,
}

#[async_trait]
impl PinnedStore for RelayPin {
    async fn read(&mut self) -> Result<SourceCut, ()> {
        let pinned = std::mem::replace(
            &mut self.pinned,
            PinnedRelay {
                config: None,
                peers: Vec::new(),
                config_exists: false,
                peers_exist: false,
                peers_unreadable: false,
            },
        );
        tokio::task::spawn_blocking(move || read_relay(pinned))
            .await
            .map_err(|_| ())?
    }
}

fn pin_relay(namespace: &Path) -> PinnedRelay {
    let config_path = namespace.join("config.json");
    let config_exists = config_path.exists();
    let config = config_exists.then(|| freeze_one("config.json".to_owned(), &config_path, false));
    let peers_path = namespace.join("peer-gateways");
    let peers_exist = peers_path.is_dir();
    let mut peers = Vec::new();
    let mut peers_unreadable = false;
    if peers_exist {
        match fs::read_dir(&peers_path) {
            Ok(entries) => {
                for entry in entries {
                    let Ok(entry) = entry else {
                        peers_unreadable = true;
                        continue;
                    };
                    let path = entry.path();
                    if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                        continue;
                    }
                    let identity = file_identity(&peers_path, &path);
                    peers.push(freeze_one(identity, &path, false));
                }
            }
            Err(_) => peers_unreadable = true,
        }
    }
    PinnedRelay {
        config,
        peers,
        config_exists,
        peers_exist,
        peers_unreadable,
    }
}

fn read_relay(pinned: PinnedRelay) -> Result<SourceCut, ()> {
    if pinned.peers_unreadable {
        return Err(());
    }
    let mut records = Vec::new();
    let mut skipped = Vec::new();
    if let Some(config) = pinned.config {
        match read_frozen(&config)? {
            FrozenRead::Unreadable => {
                skipped.push(skip(StoreKind::RelayConfiguration, "config.json")?);
            }
            FrozenRead::Bounded => skipped.push(
                SkippedRecord::new(
                    StoreKind::RelayConfiguration,
                    "config.json",
                    SkipReason::Bounded,
                )
                .map_err(|_| ())?,
            ),
            FrozenRead::Bytes(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(value) => {
                    if value.get("boat").is_some() {
                        match BoatRuntimeConfig::from_document(&bytes) {
                            Ok(runtime) => records.push(public(
                                StoreKind::RelayConfiguration,
                                "boat-runtime",
                                serde_json::to_vec(&runtime.section()).map_err(|_| ())?,
                            )?),
                            Err(_) => {
                                skipped.push(skip(StoreKind::RelayConfiguration, "boat-runtime")?)
                            }
                        }
                    }
                    if let Some(relay) = value.get("native").and_then(|native| native.get("relay"))
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
        }
    }
    for peer in pinned.peers {
        let identity = peer.identity.clone();
        match read_frozen(&peer)? {
            FrozenRead::Unreadable => {
                skipped.push(skip(StoreKind::RelayConfiguration, &identity)?);
            }
            FrozenRead::Bounded => skipped.push(
                SkippedRecord::new(
                    StoreKind::RelayConfiguration,
                    &identity,
                    SkipReason::Bounded,
                )
                .map_err(|_| ())?,
            ),
            FrozenRead::Bytes(bytes) => {
                let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                    skipped.push(skip(StoreKind::RelayConfiguration, &identity)?);
                    continue;
                };
                if let Some(relay) = value.get("relay").filter(|relay| !relay.is_null()) {
                    let encoded = serde_json::to_vec(relay).map_err(|_| ())?;
                    records.push(public(
                        StoreKind::RelayConfiguration,
                        &format!("peer-{identity}"),
                        encoded,
                    )?);
                }
            }
        }
    }
    if records.is_empty() && skipped.is_empty() && !pinned.config_exists && !pinned.peers_exist {
        return absent(StoreKind::RelayConfiguration);
    }
    finish(
        StoreKind::RelayConfiguration,
        &records.len().saturating_add(skipped.len()).to_string(),
        records,
        skipped,
    )
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
        // The registry and the receiver snapshot are taken together under
        // publication.lock. That wait stays off the async worker.
        let kind = self.kind;
        let credentials = self.credentials.clone();
        let wanted = self.wanted;
        let cut = tokio::task::spawn_blocking(move || {
            CredentialStore {
                kind,
                credentials,
                wanted,
            }
            .cut()
        })
        .await
        .map_err(|_| ())?;
        copied(cut)
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
    state: Arc<Mutex<ReceiverState>>,
    credentials: Arc<CredentialCache>,
}

enum ReceiverState {
    Cold,
    Absent,
    Open(ReceiverSnapshot),
    Failed,
}

impl Receivers {
    fn new(credentials: Arc<CredentialCache>) -> Self {
        Self {
            state: credentials.receivers.clone(),
            credentials,
        }
    }
}

#[async_trait]
impl ExportSource for Receivers {
    fn kind(&self) -> StoreKind {
        StoreKind::ReceiverAccess
    }

    async fn pin(&self) -> Result<Box<dyn PinnedStore>, ()> {
        let credentials = self.credentials.clone();
        tokio::task::spawn_blocking(move || {
            let _state = credentials.load();
        })
        .await
        .map_err(|_| ())?;
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
    namespace: PathBuf,
    facts: Arc<FactsCache>,
    credentials: Arc<CredentialCache>,
}

/// Tombstones, revoked grants, and revoked credentials committed in
/// `namespace`. Pins the metadata snapshot this read uses.
pub(crate) async fn read_committed_fences(namespace: &Path) -> Result<Vec<DeletionFence>, ()> {
    let facts = FactsCache::new(namespace.join("conversations").join("metadata.sqlite3"));
    facts.pin().await?;
    let credentials = CredentialCache::new(
        namespace.join("auth").join("credentials.v1.json"),
        namespace.join("receiver-access").join("publication.lock"),
        namespace
            .join("receiver-access")
            .join("receiver-access.sqlite3"),
    );
    NamespaceFences {
        namespace: namespace.to_path_buf(),
        facts: Arc::new(facts),
        credentials: Arc::new(credentials),
    }
    .fences()
    .await
}

#[async_trait]
impl FenceSource for NamespaceFences {
    async fn fences(&self) -> Result<Vec<DeletionFence>, ()> {
        let mut fences = Vec::new();
        let mut failed = false;
        let workers = match worker_credential_ids(&self.namespace) {
            Ok(ids) => ids,
            Err(()) => {
                failed = true;
                HashSet::new()
            }
        };
        match self.facts.fence_facts().await {
            Ok(Some(facts)) => {
                fences.extend(fences_from_facts(&facts));
            }
            Ok(None) => {}
            Err(()) => failed = true,
        }
        match self.credentials.load() {
            CredState::Read(read) => {
                // A skipped row may be a revoked credential. Leaving it out
                // would authenticate an inventory that restore could undo.
                if !read.skipped.is_empty() {
                    failed = true;
                }
                for credential in read
                    .credentials
                    .iter()
                    .filter(|credential| credential.revoked)
                {
                    let kind = enrolled_kind(credential.kind, &credential.id, &workers);
                    if let Ok(fence) =
                        DeletionFence::credential(kind, &credential.id, credential.fingerprint)
                    {
                        fences.push(fence);
                    }
                }
            }
            CredState::Absent => {}
            CredState::Unreadable => failed = true,
        }
        if failed {
            return Err(());
        }
        Ok(fences)
    }
}

/// Credential ids named by a worker enrollment. An absent directory is
/// empty. A path that is not a directory, a symlink, or a `*.json` record
/// without a usable `credentialId` fails the fence source.
fn worker_credential_ids(namespace: &Path) -> Result<HashSet<String>, ()> {
    let mut ids = HashSet::new();
    read_worker_tree(&namespace.join("workers"), &mut ids)?;
    read_worker_tree(&namespace.join("environment").join("workers"), &mut ids)?;
    Ok(ids)
}

fn read_worker_tree(root: &Path, ids: &mut HashSet<String>) -> Result<(), ()> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(());
    }
    walk_worker_dir(root, ids)
}

fn walk_worker_dir(directory: &Path, ids: &mut HashSet<String>) -> Result<(), ()> {
    for entry in fs::read_dir(directory).map_err(|_| ())? {
        let path = entry.map_err(|_| ())?.path();
        let metadata = fs::symlink_metadata(&path).map_err(|_| ())?;
        if metadata.file_type().is_symlink() {
            return Err(());
        }
        if metadata.is_dir() {
            walk_worker_dir(&path, ids)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(());
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return Err(());
        };
        if !name.ends_with(".json") {
            continue;
        }
        let value: Value =
            serde_json::from_slice(&fs::read(&path).map_err(|_| ())?).map_err(|_| ())?;
        let Some(id) = value.get("credentialId").and_then(Value::as_str) else {
            return Err(());
        };
        if id.is_empty() || id.len() > 512 || id.bytes().any(|byte| byte < 0x20) {
            return Err(());
        }
        ids.insert(id.to_owned());
    }
    Ok(())
}

/// A hosted worker and an environment peer share one registry class.
/// The enrollment file is what says this credential is a worker. A device
/// or peer id named there stays what the registry said.
fn enrolled_kind(kind: CredentialKind, id: &str, workers: &HashSet<String>) -> CredentialKind {
    if kind == CredentialKind::EnvironmentPeer && workers.contains(id) {
        CredentialKind::Worker
    } else {
        kind
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
        let Some(role) = GrantRole::parse(&revocation.capabilities) else {
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

/// A file opened at the pin. Its body is not read until admission is released,
/// and then only through [`read_frozen`], which stops at `len`.
struct FrozenFile {
    identity: String,
    body: FrozenBody,
}

enum FrozenBody {
    /// `len` is the size at the pin. A later append is past it.
    Ready {
        file: File,
        len: u64,
        secret_path: bool,
        jsonl: bool,
    },
    Bounded,
    Unreadable,
}

struct FrozenTree {
    present: bool,
    files: Vec<FrozenFile>,
}

enum FrozenRead {
    Bytes(Vec<u8>),
    Bounded,
    Unreadable,
}

fn freeze_directory(roots: &[LabeledRoot], extra_files: &[PathBuf]) -> FrozenTree {
    let mut files = Vec::new();
    let mut present = false;
    for root in roots {
        if !root.path.exists() {
            continue;
        }
        present = true;
        freeze_tree(&root.path, &root.path, root.label, &mut files);
    }
    for path in extra_files {
        if !path.exists() {
            continue;
        }
        present = true;
        files.push(freeze_one(file_name(path), path, true));
    }
    FrozenTree { present, files }
}

fn freeze_tree(root: &Path, directory: &Path, label: &str, files: &mut Vec<FrozenFile>) {
    let Ok(entries) = fs::read_dir(directory) else {
        files.push(FrozenFile {
            identity: named_identity(label, root, directory),
            body: FrozenBody::Unreadable,
        });
        return;
    };
    let mut index = 0u64;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                index += 1;
                files.push(FrozenFile {
                    identity: format!("entry-{index}"),
                    body: FrozenBody::Unreadable,
                });
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
            files.push(FrozenFile {
                identity: named_identity(label, root, &path),
                body: FrozenBody::Unreadable,
            });
            continue;
        };
        if metadata.file_type().is_symlink() {
            files.push(FrozenFile {
                identity: named_identity(label, root, &path),
                body: FrozenBody::Unreadable,
            });
            continue;
        }
        if metadata.is_dir() {
            freeze_tree(root, &path, label, files);
            continue;
        }
        files.push(freeze_one(named_identity(label, root, &path), &path, true));
    }
}

/// Open `path` and record its length. The descriptor stays at offset 0.
fn freeze_one(identity: String, path: &Path, allow_jsonl: bool) -> FrozenFile {
    let secret_path = path_sensitivity(path) == Sensitivity::Secret;
    let jsonl = allow_jsonl
        && !secret_path
        && path.extension().and_then(|ext| ext.to_str()) == Some("jsonl");
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => {
            return FrozenFile {
                identity,
                body: FrozenBody::Unreadable,
            };
        }
    };
    let len = file
        .metadata()
        .map(|metadata| metadata.len())
        .unwrap_or(u64::MAX);
    if len > PUBLIC_FILE_LIMIT {
        return FrozenFile {
            identity,
            body: FrozenBody::Bounded,
        };
    }
    FrozenFile {
        identity,
        body: FrozenBody::Ready {
            file,
            len,
            secret_path,
            jsonl,
        },
    }
}

fn read_frozen(frozen: &FrozenFile) -> Result<FrozenRead, ()> {
    match &frozen.body {
        FrozenBody::Bounded => Ok(FrozenRead::Bounded),
        FrozenBody::Unreadable => Ok(FrozenRead::Unreadable),
        FrozenBody::Ready { file, len, .. } => {
            let mut bytes = Vec::new();
            // `try_clone` keeps the pin's descriptor. `take` stops at the
            // length recorded while admission was held.
            file.try_clone()
                .map_err(|_| ())?
                .take(*len)
                .read_to_end(&mut bytes)
                .map_err(|_| ())?;
            if bytes.len() as u64 != *len {
                return Ok(FrozenRead::Unreadable);
            }
            Ok(FrozenRead::Bytes(bytes))
        }
    }
}

fn materialize(
    kind: StoreKind,
    files: Vec<FrozenFile>,
    records: &mut Vec<CapturedRecord>,
    skipped: &mut Vec<SkippedRecord>,
) {
    for file in files {
        let identity = file.identity.clone();
        let secret_path = matches!(
            file.body,
            FrozenBody::Ready {
                secret_path: true,
                ..
            }
        );
        let jsonl = matches!(file.body, FrozenBody::Ready { jsonl: true, .. });
        match read_frozen(&file) {
            Ok(FrozenRead::Bytes(bytes)) if jsonl => {
                push_jsonl_bytes(kind, &identity, bytes, records, skipped)
            }
            Ok(FrozenRead::Bytes(bytes)) => {
                push_bytes(kind, &identity, bytes, secret_path, records, skipped);
            }
            Ok(FrozenRead::Bounded) => {
                if let Ok(record) = SkippedRecord::new(kind, &identity, SkipReason::Bounded) {
                    skipped.push(record);
                }
            }
            Ok(FrozenRead::Unreadable) | Err(()) => {
                if let Ok(record) = skip(kind, &identity) {
                    skipped.push(record);
                }
            }
        }
    }
}

/// Open a regular file for a cut without reading it. Tests use this to show
/// the pin's descriptor is still at offset 0.
#[cfg(test)]
pub(crate) fn open_at_cut(path: &Path) -> std::io::Result<File> {
    let frozen = freeze_one("file".to_owned(), path, false);
    match frozen.body {
        FrozenBody::Ready { file, .. } => Ok(file),
        FrozenBody::Bounded => Err(std::io::Error::other("bounded")),
        FrozenBody::Unreadable => Err(std::io::Error::other("unreadable")),
    }
}

fn named_identity(label: &str, root: &Path, path: &Path) -> String {
    let relative = file_identity(root, path);
    if label.is_empty() {
        relative
    } else {
        format!("{label}/{relative}")
    }
}

fn push_jsonl_bytes(
    kind: StoreKind,
    identity: &str,
    bytes: Vec<u8>,
    records: &mut Vec<CapturedRecord>,
    skipped: &mut Vec<SkippedRecord>,
) {
    if content_is_secret(&bytes) {
        withhold(kind, identity, bytes, records, skipped);
        return;
    }
    let Ok(text) = String::from_utf8(bytes) else {
        if let Ok(record) = skip(kind, identity) {
            skipped.push(record);
        }
        return;
    };
    match jsonl_cut(kind, &text) {
        Ok(cut) => {
            records.extend(cut.records().iter().cloned());
            skipped.extend(cut.skipped().iter().cloned());
        }
        Err(_) => {
            if let Ok(record) = skip(kind, identity) {
                skipped.push(record);
            }
        }
    }
}

#[derive(Clone)]
enum CappedFile {
    Missing,
    Unreadable,
    Bounded,
    Bytes(Vec<u8>),
}

fn read_capped(path: &Path, limit: u64) -> CappedFile {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return CappedFile::Missing,
        Err(_) => return CappedFile::Unreadable,
    };
    let length = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    if length > limit {
        return CappedFile::Bounded;
    }
    let mut bytes = Vec::new();
    let mut limited = file.take(limit.saturating_add(1));
    if limited.read_to_end(&mut bytes).is_err() {
        return CappedFile::Unreadable;
    }
    if bytes.len() as u64 > limit {
        return CappedFile::Bounded;
    }
    CappedFile::Bytes(bytes)
}

fn push_capped_record(
    kind: StoreKind,
    identity: &str,
    file: CappedFile,
    records: &mut Vec<CapturedRecord>,
    skipped: &mut Vec<SkippedRecord>,
) -> Result<(), ()> {
    match file {
        CappedFile::Missing => Ok(()),
        CappedFile::Unreadable => {
            skipped.push(skip(kind, identity)?);
            Ok(())
        }
        CappedFile::Bounded => {
            skipped.push(SkippedRecord::new(kind, identity, SkipReason::Bounded).map_err(|_| ())?);
            Ok(())
        }
        CappedFile::Bytes(bytes) => {
            push_bytes(kind, identity, bytes, false, records, skipped);
            Ok(())
        }
    }
}

fn push_capped_jsonl(
    kind: StoreKind,
    identity: &str,
    file: CappedFile,
    records: &mut Vec<CapturedRecord>,
    skipped: &mut Vec<SkippedRecord>,
) -> Result<(), ()> {
    match file {
        CappedFile::Missing => Ok(()),
        CappedFile::Unreadable => {
            skipped.push(skip(kind, identity)?);
            Ok(())
        }
        CappedFile::Bounded => {
            skipped.push(SkippedRecord::new(kind, identity, SkipReason::Bounded).map_err(|_| ())?);
            Ok(())
        }
        CappedFile::Bytes(bytes) => {
            if content_is_secret(&bytes) {
                withhold(kind, identity, bytes, records, skipped);
                return Ok(());
            }
            let Ok(text) = String::from_utf8(bytes) else {
                skipped.push(skip(kind, identity)?);
                return Ok(());
            };
            match jsonl_cut(kind, &text) {
                Ok(cut) => {
                    records.extend(cut.records().iter().cloned());
                    skipped.extend(cut.skipped().iter().cloned());
                    Ok(())
                }
                Err(_) => {
                    skipped.push(skip(kind, identity)?);
                    Ok(())
                }
            }
        }
    }
}

fn push_bytes(
    kind: StoreKind,
    identity: &str,
    bytes: Vec<u8>,
    secret_path: bool,
    records: &mut Vec<CapturedRecord>,
    skipped: &mut Vec<SkippedRecord>,
) {
    let content_secret = content_is_secret(&bytes);
    let secret = secret_path || content_secret;
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

pub(crate) fn content_is_secret(bytes: &[u8]) -> bool {
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
    SourceCut::absent(kind).map_err(|_| ())
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
