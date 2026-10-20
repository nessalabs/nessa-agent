//! Quarantine directories, sealed inventory files, and deferred session copies.
//!
//! The application decides what may be kept. This module reads JSON records,
//! writes the partial tree, and streams one oversized conversation.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use nessa_local_storage::{create_directory, open, sync_directory, OpenMode};
use serde_json::{json, Value};

use sha2::Digest;

use crate::backup::application::{
    CutMaterializer, DecisionAuditError, ExportClock, FetchProgress, LoadedTree, MacKey,
    PreparedCut, QuarantineLayout, RestoreAudit, RestoreEvidence, SealedInventoryReader,
    SessionCut, SessionSourceError, Staging, VerificationFormat, VerificationLimits,
    VerificationSource,
};
use crate::backup::domain::{
    conversation_is_fenced, credential_is_fenced, encode_deferred, fresher_fences,
    leases_to_interrupt, metadata_omitted, normalized_root, paths_are_distinct,
    relative_path_is_safe, share_grant_is_fenced, CredentialKind, DeferredEntry, DeferredLine,
    DeletionFence, DeletionLog, Fingerprint, LeaseMark, ManifestListing, MetadataFacts,
    MetadataRow, RestoreFault, SealedFile, StoreKind, IDENTITY_BODY, MAX_NAME_BYTES, REPLAY_BODY,
};

use super::log::{authenticate_inventory, exclusive, persist};
use super::snapshot::{inspect_conversation, materialize_conversation, MaterializeError};

/// Reads a later inventory that carries its own mac line.
pub struct KeyedInventory {
    key: MacKey,
}

impl KeyedInventory {
    pub fn new(key: MacKey) -> Self {
        Self { key }
    }
}

impl SealedInventoryReader for KeyedInventory {
    fn read_later(&self, text: &str) -> Result<DeletionLog, RestoreFault> {
        authenticate_inventory(text, self.key).map_err(|_| RestoreFault::Verification)
    }
}

/// Prepares quarantine files from a bundle the caller has already verified.
pub struct DirectoryMaterializer;

impl CutMaterializer for DirectoryMaterializer {
    fn prepare(
        &self,
        bundle: &dyn VerificationSource,
        listing: &ManifestListing,
        format: &dyn VerificationFormat,
        limits: VerificationLimits,
        later: Option<&DeletionLog>,
    ) -> Result<PreparedCut, RestoreFault> {
        let cut_inventory = inventory_log(bundle, format, listing, limits)?;
        let (fences, freshness) = fresher_fences(&cut_inventory, later)?;
        let fence_list = fences
            .entries()
            .iter()
            .map(|entry| entry.fence().clone())
            .collect::<Vec<_>>();
        let mut files = Vec::new();
        let mut leases = Vec::new();
        let mut deferred = Vec::new();
        for store in listing.stores() {
            for line in store.deferred() {
                let row = deferred_line(line)?;
                if conversation_is_fenced(&row.conversation_id, &fence_list) {
                    continue;
                }
                deferred.push(row);
            }
            let mut lease_files = Vec::new();
            for record in store.records().iter().filter(|record| record.is_public()) {
                let reader = bundle
                    .record(store.store(), &record.encoded_identity())
                    .map_err(|_| RestoreFault::Verification)?;
                let body = read_reader(reader, limits)?;
                if Fingerprint::of(&body) != record.fingerprint() {
                    return Err(RestoreFault::Verification);
                }
                if store.store() == StoreKind::LeaseRecords {
                    lease_files.push((record.encoded_identity(), body));
                    continue;
                }
                if omit_record(store.store(), record.identity(), &body, &fence_list)? {
                    continue;
                }
                files.push((
                    format!(
                        "records/{}/{}",
                        store.store().as_str(),
                        record.encoded_identity()
                    ),
                    body,
                ));
            }
            lease_files.sort_by(|left, right| {
                line_rank(&left.0)
                    .cmp(&line_rank(&right.0))
                    .then_with(|| left.0.cmp(&right.0))
            });
            let mut folded = Vec::new();
            let mut per_file = Vec::new();
            for (identity, body) in lease_files {
                let text = std::str::from_utf8(&body)
                    .map_err(|_| RestoreFault::Unreadable)?
                    .to_owned();
                let events = lease_events(&text)?;
                folded.extend(events.iter().cloned());
                per_file.push((identity, text, events));
            }
            let open = leases_to_interrupt(&folded);
            let mut last_open = Vec::new();
            for (index, (_, _, events)) in per_file.iter().enumerate() {
                for (id, mark) in events {
                    if *mark != LeaseMark::Opened || !open.iter().any(|existing| existing == id) {
                        continue;
                    }
                    if let Some(slot) = last_open.iter_mut().find(|(existing, _)| existing == id) {
                        *slot = (id.clone(), index);
                    } else {
                        last_open.push((id.clone(), index));
                    }
                }
            }
            for (index, (identity, text, _)) in per_file.into_iter().enumerate() {
                let here = last_open
                    .iter()
                    .filter(|(_, at)| *at == index)
                    .map(|(id, _)| id.clone())
                    .collect::<Vec<_>>();
                files.push((
                    format!("records/{}/{}", StoreKind::LeaseRecords.as_str(), identity),
                    interrupt_leases(&text, &here)?,
                ));
            }
            if store.store() == StoreKind::LeaseRecords {
                leases = open;
            }
        }
        files.push((
            "deferred.txt".into(),
            encode_deferred(&deferred)?.into_bytes(),
        ));
        files.push(("replay.blocked".into(), REPLAY_BODY.as_bytes().to_vec()));
        files.push(("identity.txt".into(), IDENTITY_BODY.as_bytes().to_vec()));
        Ok(PreparedCut {
            files,
            leases,
            deferred,
            freshness,
        })
    }
}

fn deferred_line(entry: &DeferredEntry) -> Result<DeferredLine, RestoreFault> {
    Ok(DeferredLine {
        store: entry.store(),
        conversation_id: entry.conversation_id().to_owned(),
        size: entry.size(),
        digest: entry.digest(),
    })
}

fn inventory_log(
    bundle: &dyn VerificationSource,
    format: &dyn VerificationFormat,
    listing: &ManifestListing,
    limits: VerificationLimits,
) -> Result<DeletionLog, RestoreFault> {
    let store = listing
        .stores()
        .iter()
        .find(|store| store.store() == StoreKind::DeletionInventory)
        .ok_or(RestoreFault::Verification)?;
    let Some(record) = store.records().iter().find(|record| record.is_public()) else {
        return Ok(DeletionLog::default());
    };
    let reader = bundle
        .record(store.store(), &record.encoded_identity())
        .map_err(|_| RestoreFault::Verification)?;
    let body = read_reader(reader, limits)?;
    if Fingerprint::of(&body) != record.fingerprint() {
        return Err(RestoreFault::Verification);
    }
    let text = std::str::from_utf8(&body).map_err(|_| RestoreFault::Verification)?;
    format
        .inventory(text)
        .map_err(|_| RestoreFault::Verification)
}

fn omit_record(
    store: StoreKind,
    identity: &str,
    body: &[u8],
    fences: &[DeletionFence],
) -> Result<bool, RestoreFault> {
    if store == StoreKind::ShareGrants {
        let grant = read_grant(body).map_err(|_| RestoreFault::Unreadable)?;
        return Ok(share_grant_is_fenced(
            &grant.conversation_id,
            &grant.receiver_id,
            &grant.credential_id,
            fences,
        ));
    }
    if let Some(kind) = credential_store(store) {
        let credential = read_credential(body).map_err(|_| RestoreFault::Unreadable)?;
        if credential.kind != kind {
            return Err(RestoreFault::Unreadable);
        }
        return Ok(credential_is_fenced(kind, &credential.id, fences));
    }
    if store == StoreKind::ConversationMetadata {
        let metadata = read_metadata(identity, body).map_err(|_| RestoreFault::Unreadable)?;
        return metadata_omitted(
            MetadataFacts {
                row: metadata.row,
                conversation_id: metadata.conversation_id.as_deref(),
            },
            fences,
        );
    }
    Ok(false)
}

fn credential_store(store: StoreKind) -> Option<CredentialKind> {
    match store {
        StoreKind::DeviceCredentials => Some(CredentialKind::Device),
        StoreKind::PeerCredentials => Some(CredentialKind::Peer),
        StoreKind::EnvironmentPeerCredentials => Some(CredentialKind::EnvironmentPeer),
        StoreKind::WorkerCredentials => Some(CredentialKind::Worker),
        _ => None,
    }
}

struct GrantFields {
    conversation_id: String,
    receiver_id: String,
    credential_id: String,
}

fn read_grant(bytes: &[u8]) -> Result<GrantFields, ()> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| ())?;
    let conversation_id = text_field(&value, "conversationId")?;
    let receiver_id = value
        .get("receiverId")
        .and_then(Value::as_str)
        .or_else(|| value.get("principalId").and_then(Value::as_str))
        .ok_or(())?
        .to_owned();
    Ok(GrantFields {
        conversation_id,
        receiver_id,
        credential_id: text_field(&value, "credentialId")?,
    })
}

struct CredentialFields {
    id: String,
    kind: CredentialKind,
}

fn read_credential(bytes: &[u8]) -> Result<CredentialFields, ()> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| ())?;
    Ok(CredentialFields {
        id: text_field(&value, "id")?,
        kind: CredentialKind::parse(&text_field(&value, "kind")?).ok_or(())?,
    })
}

struct MetadataFields {
    row: MetadataRow,
    conversation_id: Option<String>,
}

fn read_metadata(identity: &str, bytes: &[u8]) -> Result<MetadataFields, ()> {
    let (row, prefix, field) = if let Some(rest) = identity.strip_prefix("conversation-") {
        (MetadataRow::Conversation, rest, "id")
    } else if let Some(rest) = identity.strip_prefix("summary-") {
        (MetadataRow::Summary, rest, "conversationId")
    } else if let Some(rest) = identity.strip_prefix("tombstone-") {
        (MetadataRow::Tombstone, rest, "conversationId")
    } else {
        return Err(());
    };
    let value: Value = serde_json::from_slice(bytes).map_err(|_| ())?;
    let conversation_id = text_field(&value, field)?;
    if conversation_id != prefix {
        return Err(());
    }
    Ok(MetadataFields {
        row,
        conversation_id: Some(conversation_id),
    })
}

fn text_field(value: &Value, name: &str) -> Result<String, ()> {
    value
        .get(name)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .ok_or(())
}

fn line_rank(identity: &str) -> (u8, u64) {
    identity
        .strip_prefix("line-")
        .and_then(|rest| rest.parse::<u64>().ok())
        .map(|number| (0, number))
        .unwrap_or((1, 0))
}

fn lease_events(text: &str) -> Result<Vec<(String, LeaseMark)>, RestoreFault> {
    let mut events = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line).map_err(|_| RestoreFault::Unreadable)?;
        let kind = value
            .get("kind")
            .and_then(Value::as_str)
            .ok_or(RestoreFault::Unreadable)?;
        let lease = value.get("lease").and_then(Value::as_str);
        let mark = match kind {
            "granted" | "command_granted" => LeaseMark::Opened,
            "ended" => LeaseMark::Closed,
            _ => LeaseMark::Noted,
        };
        if matches!(mark, LeaseMark::Opened | LeaseMark::Closed) {
            let lease = lease.ok_or(RestoreFault::Unreadable)?;
            if lease.is_empty() || lease.len() > MAX_NAME_BYTES {
                return Err(RestoreFault::Unreadable);
            }
            events.push((lease.to_owned(), mark));
        }
    }
    Ok(events)
}

fn interrupt_leases(text: &str, open: &[String]) -> Result<Vec<u8>, RestoreFault> {
    if open.is_empty() {
        return Ok(text.as_bytes().to_vec());
    }
    let mut out = text.to_owned();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    for id in open {
        out.push_str(&ended_lease_line(id)?);
        out.push('\n');
    }
    Ok(out.into_bytes())
}

fn ended_lease_line(lease_id: &str) -> Result<String, RestoreFault> {
    if lease_id.is_empty() || lease_id.bytes().any(|byte| byte < 0x20) {
        return Err(RestoreFault::Unreadable);
    }
    serde_json::to_string(&json!({
        "kind": "ended",
        "lease": lease_id,
        "cleanup": {"outcome": "uncertain"},
        "cause": "lost"
    }))
    .map_err(|_| RestoreFault::Unreadable)
}

fn read_reader(
    mut reader: Box<dyn Read + '_>,
    limits: VerificationLimits,
) -> Result<Vec<u8>, RestoreFault> {
    read_limited(&mut reader, limits.total_body_bytes)
}

fn read_limited(reader: &mut dyn Read, limit: u64) -> Result<Vec<u8>, RestoreFault> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let remaining = limit.saturating_sub(bytes.len() as u64);
        let request = usize::try_from(remaining.saturating_add(1).min(buffer.len() as u64))
            .map_err(|_| RestoreFault::Verification)?;
        let count = reader
            .read(&mut buffer[..request])
            .map_err(|_| RestoreFault::Unreadable)?;
        if count as u64 > remaining {
            return Err(RestoreFault::Verification);
        }
        if count == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}

/// The real quarantine directory. A partial tree is removed if it is not finished.
pub struct DirectoryQuarantine;

impl QuarantineLayout for DirectoryQuarantine {
    fn begin(&self, quarantine: &Path, live: &Path) -> Result<Box<dyn Staging>, RestoreFault> {
        paths_are_distinct(quarantine, live)?;
        if symlink_exists(quarantine)? {
            return Err(RestoreFault::QuarantineOccupied);
        }
        let partial = partial_path(quarantine)?;
        if partial == live {
            return Err(RestoreFault::Paths);
        }
        let _ = fs::remove_dir_all(&partial);
        create_directory(&partial).map_err(|_| RestoreFault::Unreadable)?;
        Ok(Box::new(PartialTree {
            partial,
            quarantine: quarantine.to_path_buf(),
            armed: true,
        }))
    }

    fn load(&self, root: &Path) -> Result<LoadedTree, RestoreFault> {
        let journal_path = root.join("journal.txt");
        if !journal_path.is_file() {
            return Err(RestoreFault::Unreadable);
        }
        let journal = fs::read_to_string(&journal_path).map_err(|_| RestoreFault::Unreadable)?;
        let mut files = Vec::new();
        let mut extras = Vec::new();
        walk(root, root, &mut files, &mut extras)?;
        Ok(LoadedTree {
            journal,
            files,
            extras,
        })
    }

    fn promote(&self, quarantine: &Path, live: &Path) -> Result<(), RestoreFault> {
        if symlink_exists(live)? {
            return Err(RestoreFault::LiveOccupied);
        }
        fs::rename(quarantine, live).map_err(|_| RestoreFault::Rename)?;
        if let Some(parent) = live.parent() {
            let _ = sync_directory(parent);
        }
        Ok(())
    }

    fn remove_tree(&self, path: &Path) -> Result<(), RestoreFault> {
        if path.exists() {
            fs::remove_dir_all(path).map_err(|_| RestoreFault::Unreadable)?;
        }
        Ok(())
    }

    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        fs::read(path).map_err(|_| RestoreFault::Unreadable)
    }

    fn session_temp(&self, root: &Path, relative: &str) -> Result<PathBuf, RestoreFault> {
        let final_path = resolve(root, relative)?;
        let parent = final_path.parent().ok_or(RestoreFault::Unreadable)?;
        create_directory(parent).map_err(|_| RestoreFault::Unreadable)?;
        let mut temp = final_path.as_os_str().to_owned();
        temp.push(".partial");
        Ok(PathBuf::from(temp))
    }

    fn publish_file(&self, temp: &Path, final_path: &Path) -> Result<(), RestoreFault> {
        if symlink_exists(final_path)? {
            return Err(RestoreFault::SessionDiffers);
        }
        fs::rename(temp, final_path).map_err(|_| RestoreFault::Rename)?;
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> Result<(), RestoreFault> {
        if path.exists() {
            fs::remove_file(path).map_err(|_| RestoreFault::Unreadable)?;
        }
        Ok(())
    }

    fn exists(&self, path: &Path) -> Result<bool, RestoreFault> {
        symlink_exists(path)
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, RestoreFault> {
        path.canonicalize().map_err(|_| RestoreFault::Unreadable)
    }

    fn anchor(&self, path: &Path) -> Result<PathBuf, RestoreFault> {
        let normal = normalized_root(path)?;
        let mut prefix = normal.clone();
        let mut rest = Vec::new();
        while !prefix.exists() {
            let Some(name) = prefix.file_name().map(|name| name.to_os_string()) else {
                return Err(RestoreFault::Paths);
            };
            rest.push(name);
            if !prefix.pop() {
                return Err(RestoreFault::Paths);
            }
        }
        let mut anchored = prefix
            .canonicalize()
            .map_err(|_| RestoreFault::Unreadable)?;
        for name in rest.into_iter().rev() {
            anchored.push(name);
        }
        Ok(anchored)
    }

    fn session_cut(&self, path: &Path, conversation_id: &str) -> Result<SessionCut, RestoreFault> {
        let stream =
            inspect_conversation(path, conversation_id).map_err(|_| RestoreFault::Tamper)?;
        Ok(SessionCut {
            size: stream.size,
            digest: Fingerprint::from_digest(stream.digest),
        })
    }
}

struct PartialTree {
    partial: PathBuf,
    quarantine: PathBuf,
    armed: bool,
}

impl Staging for PartialTree {
    fn write(&mut self, relative: &str, bytes: &[u8]) -> Result<(), RestoreFault> {
        let path = resolve(&self.partial, relative)?;
        if let Some(parent) = path.parent() {
            create_directory(parent).ok();
            if !parent.is_dir() {
                fs::create_dir_all(parent).map_err(|_| RestoreFault::Unreadable)?;
            }
        }
        let mut file = open(&path, OpenMode::CreateNew).map_err(|_| RestoreFault::Unreadable)?;
        file.write_all(bytes)
            .map_err(|_| RestoreFault::Unreadable)?;
        file.sync_all().map_err(|_| RestoreFault::Unreadable)?;
        Ok(())
    }

    fn finish(mut self: Box<Self>, journal: &str) -> Result<(), RestoreFault> {
        self.write("journal.txt", journal.as_bytes())?;
        sync_tree(&self.partial)?;
        if symlink_exists(&self.quarantine)? {
            return Err(RestoreFault::QuarantineOccupied);
        }
        fs::rename(&self.partial, &self.quarantine).map_err(|_| RestoreFault::Rename)?;
        if let Some(parent) = self.quarantine.parent() {
            let _ = sync_directory(parent);
        }
        self.armed = false;
        Ok(())
    }
}

impl Drop for PartialTree {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.partial);
        }
    }
}

fn walk(
    root: &Path,
    directory: &Path,
    files: &mut Vec<SealedFile>,
    extras: &mut Vec<String>,
) -> Result<(), RestoreFault> {
    for entry in fs::read_dir(directory).map_err(|_| RestoreFault::Unreadable)? {
        let entry = entry.map_err(|_| RestoreFault::Unreadable)?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).map_err(|_| RestoreFault::Unreadable)?;
        if meta.file_type().is_symlink() {
            return Err(RestoreFault::Tamper);
        }
        if meta.is_dir() {
            walk(root, &path, files, extras)?;
            continue;
        }
        if !meta.is_file() {
            return Err(RestoreFault::Tamper);
        }
        let relative = relative_to(root, &path)?;
        if relative == "journal.txt" {
            continue;
        }
        let fingerprint = hash_file(&path)?;
        if relative.starts_with("sessions/") {
            extras.push(relative);
        } else {
            files.push(SealedFile {
                path: relative,
                fingerprint,
            });
        }
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<Fingerprint, RestoreFault> {
    let mut file = File::open(path).map_err(|_| RestoreFault::Unreadable)?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| RestoreFault::Unreadable)?;
        if count == 0 {
            break;
        }
        Digest::update(&mut hasher, &buffer[..count]);
    }
    Ok(Fingerprint::from_digest(hasher.finalize().into()))
}

fn relative_to(root: &Path, path: &Path) -> Result<String, RestoreFault> {
    let relative = path.strip_prefix(root).map_err(|_| RestoreFault::Tamper)?;
    let mut text = String::new();
    for (index, part) in relative.components().enumerate() {
        let std::path::Component::Normal(part) = part else {
            return Err(RestoreFault::Tamper);
        };
        if index > 0 {
            text.push('/');
        }
        text.push_str(part.to_str().ok_or(RestoreFault::Tamper)?);
    }
    if !relative_path_is_safe(&text) {
        return Err(RestoreFault::Tamper);
    }
    Ok(text)
}

fn resolve(root: &Path, relative: &str) -> Result<PathBuf, RestoreFault> {
    if !relative_path_is_safe(relative) {
        return Err(RestoreFault::Tamper);
    }
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        path.push(part);
    }
    Ok(path)
}

fn partial_path(quarantine: &Path) -> Result<PathBuf, RestoreFault> {
    static ATTEMPT: AtomicU64 = AtomicU64::new(1);
    let name = quarantine
        .file_name()
        .ok_or(RestoreFault::Paths)?
        .to_os_string();
    let mut staged = name;
    staged.push(format!(
        ".partial-{}-{}",
        std::process::id(),
        ATTEMPT.fetch_add(1, Ordering::Relaxed)
    ));
    let parent = quarantine.parent().ok_or(RestoreFault::Paths)?;
    if !parent.is_dir() {
        return Err(RestoreFault::Unreadable);
    }
    Ok(parent.join(staged))
}

fn symlink_exists(path: &Path) -> Result<bool, RestoreFault> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(RestoreFault::Unreadable),
    }
}

fn sync_tree(dir: &Path) -> Result<(), RestoreFault> {
    let mut children = Vec::new();
    for entry in fs::read_dir(dir).map_err(|_| RestoreFault::Unreadable)? {
        let path = entry.map_err(|_| RestoreFault::Unreadable)?.path();
        if path.is_dir() {
            children.push(path);
        }
    }
    for child in children {
        sync_tree(&child)?;
    }
    sync_directory(dir).map_err(|_| RestoreFault::Unreadable)
}

/// Append-only restore audit. The clock and the record id belong to this file.
pub struct FileRestoreAudit<C> {
    directory: PathBuf,
    path: PathBuf,
    clock: C,
    gate: Mutex<()>,
}

impl<C: ExportClock> FileRestoreAudit<C> {
    pub fn open(directory: impl Into<PathBuf>, clock: C) -> std::io::Result<Self> {
        let directory = directory.into();
        create_directory(&directory)?;
        Ok(Self {
            path: directory.join("restore.jsonl"),
            directory,
            clock,
            gate: Mutex::new(()),
        })
    }
}

impl<C: ExportClock> RestoreAudit for FileRestoreAudit<C> {
    fn append(&self, evidence: &RestoreEvidence<'_>) -> Result<(), DecisionAuditError> {
        let _gate = self
            .gate
            .lock()
            .map_err(|_| DecisionAuditError::NotStored)?;
        let _file = exclusive(&self.directory).map_err(|_| DecisionAuditError::NotStored)?;
        let next = next_sequence(&self.path).map_err(|_| DecisionAuditError::NotStored)?;
        let line = serde_json::to_string(&json!({
            "recordId": format!("restore-{next}"),
            "kind": "restore",
            "target": evidence.target,
            "before": evidence.before,
            "after": evidence.after,
            "cause": evidence.cause,
            "initiator": evidence.initiator,
            "decision": evidence.decision,
            "policies": evidence.policies,
            "attestations": evidence.attestations,
            "observedAtMs": self.clock.unix_ms(),
        }))
        .map_err(|_| DecisionAuditError::NotStored)?;
        let mut text = if self.path.is_file() {
            fs::read_to_string(&self.path).map_err(|_| DecisionAuditError::NotStored)?
        } else {
            String::new()
        };
        text.push_str(&line);
        text.push('\n');
        persist(&self.directory, &self.path, &text).map_err(|_| DecisionAuditError::NotStored)
    }
}

fn next_sequence(path: &Path) -> std::io::Result<u64> {
    if !path.exists() {
        return Ok(1);
    }
    let text = fs::read_to_string(path)?;
    Ok(text.lines().filter(|line| !line.is_empty()).count() as u64 + 1)
}

/// Streams one conversation out of a session database the caller still has.
pub struct SqliteDeferredSource {
    source: PathBuf,
}

impl SqliteDeferredSource {
    pub fn new(source: impl Into<PathBuf>) -> Self {
        Self {
            source: source.into(),
        }
    }
}

impl crate::backup::application::DeferredSessionSource for SqliteDeferredSource {
    fn write_session(
        &self,
        conversation_id: &str,
        destination: &Path,
        progress: &mut dyn FetchProgress,
    ) -> Result<(), SessionSourceError> {
        let mut report = |read, total| progress.report(read, total);
        materialize_conversation(&self.source, conversation_id, destination, &mut report)
            .map(|_| ())
            .map_err(materialize_error)
    }

    fn inspect(
        &self,
        conversation_id: &str,
        path: &Path,
    ) -> Result<SessionCut, SessionSourceError> {
        let stream = inspect_conversation(path, conversation_id).map_err(materialize_error)?;
        Ok(SessionCut {
            size: stream.size,
            digest: Fingerprint::from_digest(stream.digest),
        })
    }
}

fn materialize_error(error: MaterializeError) -> SessionSourceError {
    match error {
        MaterializeError::Missing => SessionSourceError::Missing,
        MaterializeError::Interrupted => SessionSourceError::Interrupted,
        MaterializeError::Unreadable => SessionSourceError::Unreadable,
    }
}

#[test]
fn a_restore_audit_waits_for_the_directory_lock() {
    use std::sync::mpsc;
    use std::time::Duration;

    struct Clock;
    impl ExportClock for Clock {
        fn unix_ms(&self) -> u64 {
            1
        }
    }

    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("audit");
    let first = FileRestoreAudit::open(&directory, Clock).unwrap();
    let second = FileRestoreAudit::open(&directory, Clock).unwrap();
    let held = exclusive(&directory).unwrap();
    let (started, ready) = mpsc::channel();
    let (finished, result) = mpsc::channel();
    std::thread::spawn(move || {
        started.send(()).unwrap();
        let stored = second.append(&RestoreEvidence {
            target: "live",
            before: "verified",
            after: "live",
            cause: "accepted",
            initiator: "person",
            decision: "effect",
            policies: &[],
            attestations: "lease-1 stopped\n",
        });
        finished.send(stored).unwrap();
    });
    ready.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(
        matches!(
            result.recv_timeout(Duration::from_secs(1)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ),
        "a second adapter must wait for the directory lock"
    );
    drop(held);
    result
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    first
        .append(&RestoreEvidence {
            target: "quarantine",
            before: "absent",
            after: "verified",
            cause: "sealed",
            initiator: "person",
            decision: "effect",
            policies: &[],
            attestations: "",
        })
        .unwrap();
    let text = fs::read_to_string(directory.join("restore.jsonl")).unwrap();
    assert!(text.contains("\"recordId\":\"restore-1\""));
    assert!(text.contains("\"recordId\":\"restore-2\""));
    assert!(text.contains("lease-1 stopped"));
    assert!(text.contains("sealed"));
}

/// The ended line is the environment ledger's `ended` record.
#[cfg(all(test, unix))]
#[test]
fn an_interrupted_lease_line_is_the_ledger_ended_record() {
    use crate::env_serve::application::{EndCause, LedgerEntry};
    use nessa_protocol::lease::Cleanup;

    let line = ended_lease_line("lease-1").unwrap();
    let parsed: Value = serde_json::from_str(&line).unwrap();
    let entry = LedgerEntry::Ended {
        lease: "lease-1".into(),
        cleanup: Cleanup::Uncertain,
        cause: EndCause::Lost,
    };
    let expected = serde_json::to_value(&entry).unwrap();
    assert_eq!(parsed, expected);
}
