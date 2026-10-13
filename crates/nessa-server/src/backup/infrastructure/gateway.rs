//! Read one namespace into the writer inventory while admission is held.
//!
//! A local outbox is not read. Secret paths are fingerprinted. The credential
//! registry contributes public facts only.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;

use super::credentials::{read_credentials, CredentialRead};
use super::jsonl::jsonl_cut;
use super::sensitivity::path_sensitivity;
use crate::backup::application::{
    capture_export, Authenticator, DeletionLogStore, ExportClock, ExportFailure, ExportRequest,
    ExportSource, FenceSource, ManifestPublisher, MutationAdmission,
};
use crate::backup::domain::{
    CapturedRecord, CredentialKind, DeletionFence, GrantRole, Sensitivity, SkipReason,
    SkippedRecord, SourceCut, StoreKind,
};
use crate::conversation::infrastructure::{
    export_receivers, ExportFacts, ExportSkip, LocalConversationStore, NamedRecord,
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
    let canonical = Arc::new(FileCache::new(
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
        Box::new(Receivers {
            path: namespace
                .join("receiver-access")
                .join("receiver-access.sqlite3"),
        }),
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
    slot: Mutex<Option<Result<Option<ExportFacts>, ()>>>,
}

impl FactsCache {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            slot: Mutex::new(None),
        }
    }

    async fn load(&self) -> Result<Option<ExportFacts>, ()> {
        if let Some(cached) = self.slot.lock().map_err(|_| ())?.clone() {
            return cached;
        }
        if !self.path.is_file() {
            *self.slot.lock().map_err(|_| ())? = Some(Ok(None));
            return Ok(None);
        }
        let path = self.path.clone();
        let store = tokio::task::spawn_blocking(move || LocalConversationStore::open(&path))
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?;
        let facts = store.export_facts().await.map_err(|_| ())?;
        *self.slot.lock().map_err(|_| ())? = Some(Ok(Some(facts.clone())));
        Ok(Some(facts))
    }
}

struct FileCache {
    path: PathBuf,
    slot: Mutex<Option<FileState>>,
}

#[derive(Clone)]
enum FileState {
    Absent,
    Bytes(Vec<u8>),
    Unreadable,
}

impl FileCache {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            slot: Mutex::new(None),
        }
    }

    fn load(&self) -> FileState {
        if let Some(cached) = self.slot.lock().ok().and_then(|slot| slot.clone()) {
            return cached;
        }
        let state = if !self.path.exists() {
            FileState::Absent
        } else {
            match fs::read(&self.path) {
                Ok(bytes) => FileState::Bytes(bytes),
                Err(_) => FileState::Unreadable,
            }
        };
        if let Ok(mut slot) = self.slot.lock() {
            *slot = Some(state.clone());
        }
        state
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
                    Err(()) => CredState::Unreadable,
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
    files: Arc<FileCache>,
}

#[async_trait]
impl ExportSource for CanonicalFile {
    fn kind(&self) -> StoreKind {
        StoreKind::CanonicalRecords
    }

    async fn capture(&self) -> Result<SourceCut, ()> {
        match self.files.load() {
            FileState::Absent => absent(StoreKind::CanonicalRecords),
            FileState::Unreadable => skipped_file(StoreKind::CanonicalRecords, "records.sqlite3"),
            FileState::Bytes(bytes) => {
                let record = public(StoreKind::CanonicalRecords, "records.sqlite3", bytes)?;
                finish(StoreKind::CanonicalRecords, "file", vec![record], vec![])
            }
        }
    }
}

struct NotesFile {
    files: Arc<FileCache>,
}

#[async_trait]
impl ExportSource for NotesFile {
    fn kind(&self) -> StoreKind {
        StoreKind::Notes
    }

    async fn capture(&self) -> Result<SourceCut, ()> {
        // Notes live in the canonical session file. Naming each one waits on
        // an event-stream export. The watermark is that file's fingerprint
        // when it could be read, so the cut shows they were taken together.
        match self.files.load() {
            FileState::Absent => absent(StoreKind::Notes),
            FileState::Unreadable => skipped_file(StoreKind::Notes, "records.sqlite3"),
            FileState::Bytes(bytes) => {
                let fingerprint = crate::backup::domain::Fingerprint::of(&bytes);
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

    async fn capture(&self) -> Result<SourceCut, ()> {
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

    async fn capture(&self) -> Result<SourceCut, ()> {
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

    async fn capture(&self) -> Result<SourceCut, ()> {
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
            if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                push_jsonl(self.kind, path, &mut records, &mut skipped);
            } else {
                push_path(self.kind, identity, path, &mut records, &mut skipped);
            }
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

    async fn capture(&self) -> Result<SourceCut, ()> {
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

    async fn capture(&self) -> Result<SourceCut, ()> {
        if !self.path.exists() {
            return absent(self.kind);
        }
        let text = fs::read_to_string(&self.path).map_err(|_| ())?;
        jsonl_cut(self.kind, &text).map_err(|_| ())
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

    async fn capture(&self) -> Result<SourceCut, ()> {
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

    async fn capture(&self) -> Result<SourceCut, ()> {
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
}

#[async_trait]
impl ExportSource for Receivers {
    fn kind(&self) -> StoreKind {
        StoreKind::ReceiverAccess
    }

    async fn capture(&self) -> Result<SourceCut, ()> {
        if !self.path.is_file() {
            return absent(StoreKind::ReceiverAccess);
        }
        let (watermark, rows, skipped_ids) = export_receivers(&self.path)?;
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

    async fn capture(&self) -> Result<SourceCut, ()> {
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
        match self.facts.load().await {
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

fn fences_from_facts(facts: &ExportFacts) -> Vec<DeletionFence> {
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

fn push_jsonl(
    kind: StoreKind,
    path: &Path,
    records: &mut Vec<CapturedRecord>,
    skipped: &mut Vec<SkippedRecord>,
) {
    let Ok(text) = fs::read_to_string(path) else {
        if let Ok(record) = skip(kind, "jsonl") {
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
            if let Ok(record) = skip(kind, "jsonl") {
                skipped.push(record);
            }
        }
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
    let secret = path_sensitivity(path) == Sensitivity::Secret || json_has_secret(&bytes);
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

fn json_has_secret(bytes: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return false;
    };
    secret_key(&value)
}

fn secret_key(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(key, child)| {
            matches!(
                key.as_str(),
                "verifier" | "privateKey" | "private_key" | "token" | "secret"
            ) || secret_key(child)
        }),
        Value::Array(items) => items.iter().any(secret_key),
        Value::String(text) => {
            text.contains("BEGIN PRIVATE KEY") || text.contains("BEGIN OPENSSH PRIVATE KEY")
        }
        _ => false,
    }
}

fn file_identity(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
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
