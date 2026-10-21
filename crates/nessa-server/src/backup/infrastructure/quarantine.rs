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
    PreparedCut, Promoted, PublicationIds, QuarantineLayout, RestoreAudit, RestoreEvidence,
    RestoreHold, SealedInventoryReader, SessionCut, SessionSourceError, Staging,
    VerificationFormat, VerificationLimits, VerificationSource,
};
use crate::backup::domain::{
    conversation_is_fenced, credential_is_fenced, encode_deferred, fresher_fences,
    leases_to_interrupt, metadata_omitted, normalized_root, paths_are_distinct,
    relative_path_is_safe, restore_lock_path, session_sidecar, share_grant_is_fenced,
    CredentialKind, DeferredEntry, DeferredLine, DeletionFence, DeletionLog, FenceKind,
    Fingerprint, LeaseMark, ManifestListing, MetadataFacts, MetadataRow, RestoreFault, SealedFile,
    StoreKind, IDENTITY_BODY, MAX_NAME_BYTES, REPLAY_BODY,
};

use super::log::{authenticate_inventory, exclusive, persist};
use super::snapshot::{
    inspect_conversation, materialize_conversation, without_fenced_conversations, MaterializeError,
};

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
                let body = match session_without_fenced(store.store(), &body, &fence_list)? {
                    Some(filtered) => filtered,
                    None => body,
                };
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

fn session_without_fenced(
    store: StoreKind,
    body: &[u8],
    fences: &[DeletionFence],
) -> Result<Option<Vec<u8>>, RestoreFault> {
    if !matches!(store, StoreKind::CanonicalRecords | StoreKind::Notes) {
        return Ok(None);
    }
    let fenced = fences
        .iter()
        .filter_map(|fence| match fence.kind() {
            FenceKind::Conversation { id } => Some(id.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    // A notes row that is not a database is a note. A canonical body is the
    // session image, so an unreadable one cannot be sealed while a fence applies.
    if fenced.is_empty() || (store == StoreKind::Notes && !body.starts_with(b"SQLite format 3")) {
        return Ok(None);
    }
    without_fenced_conversations(body, &fenced).map_err(|_| RestoreFault::FencedBody)
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
            grant.policy_revision,
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
    /// Absent when the exported row does not name an epoch. A fence then
    /// still matches, because a newer re-grant cannot be proved.
    policy_revision: Option<u64>,
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
    let policy_revision = match value.get("policyRevision") {
        None | Some(Value::Null) => None,
        Some(Value::Number(number)) => Some(number.as_u64().ok_or(())?),
        Some(_) => return Err(()),
    };
    Ok(GrantFields {
        conversation_id,
        receiver_id,
        credential_id: text_field(&value, "credentialId")?,
        policy_revision,
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
        // The same identity as the two roots. A case alias of this directory
        // is the live path on Windows and macOS, and must not be deleted.
        if paths_are_distinct(&partial, live).is_err() {
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
        let mut helpers = Vec::new();
        walk(root, root, &mut files, &mut extras, &mut helpers)?;
        Ok(LoadedTree {
            journal,
            files,
            extras,
            helpers,
        })
    }

    fn promote(&self, quarantine: &Path, live: &Path) -> Result<Promoted, RestoreFault> {
        if symlink_exists(live)? {
            return Err(RestoreFault::LiveOccupied);
        }
        let from_parent = quarantine.parent().map(Path::to_path_buf);
        let to_parent = live.parent().map(Path::to_path_buf);
        fs::rename(quarantine, live).map_err(|_| RestoreFault::Rename)?;
        // The rename already made the tree live. A parent that cannot be
        // synced does not put it back, and does not make the promotion a
        // refusal.
        let mut durable = true;
        if let Some(parent) = to_parent.as_deref() {
            if sync_directory(parent).is_err() {
                durable = false;
            }
        }
        if let Some(parent) = from_parent.as_deref() {
            if to_parent.as_deref() != Some(parent) && sync_directory(parent).is_err() {
                durable = false;
            }
        }
        let marker = live.join("accept-ready");
        if marker.exists() {
            let _ = fs::remove_file(marker);
        }
        Ok(Promoted { durable })
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
        // Durable bytes, then the name. A read-only open cannot flush on
        // Windows, so the writable handle is taken before the rename.
        sync_writable(temp)?;
        fs::rename(temp, final_path).map_err(|_| RestoreFault::Rename)?;
        if let Some(parent) = final_path.parent() {
            sync_directory(parent).map_err(|_| RestoreFault::Unreadable)?;
        }
        Ok(())
    }

    fn note_publication(&self, session: &Path, publication: &str) -> Result<(), RestoreFault> {
        if publication.is_empty() || publication.bytes().any(|byte| byte <= b' ') {
            return Err(RestoreFault::Unreadable);
        }
        let path = publication_file(session);
        if path.exists() {
            fs::remove_file(&path).map_err(|_| RestoreFault::Unreadable)?;
        }
        let mut file = open(&path, OpenMode::CreateNew).map_err(|_| RestoreFault::Unreadable)?;
        file.write_all(publication.as_bytes())
            .map_err(|_| RestoreFault::Unreadable)?;
        file.write_all(b"\n")
            .map_err(|_| RestoreFault::Unreadable)?;
        file.sync_all().map_err(|_| RestoreFault::Unreadable)?;
        if let Some(parent) = path.parent() {
            sync_directory(parent).map_err(|_| RestoreFault::Unreadable)?;
        }
        Ok(())
    }

    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault> {
        let path = publication_file(session);
        if !path.is_file() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path).map_err(|_| RestoreFault::Unreadable)?;
        let id = text.trim_end_matches(['\n', '\r']);
        if id.is_empty() || id.bytes().any(|byte| byte <= b' ') {
            return Err(RestoreFault::Tamper);
        }
        Ok(Some(id.to_owned()))
    }

    fn remove_publication(&self, session: &Path) -> Result<(), RestoreFault> {
        self.remove_file(&publication_file(session))
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
        let mut anchored = plain_absolute(
            prefix
                .canonicalize()
                .map_err(|_| RestoreFault::Unreadable)?,
        );
        for name in rest.into_iter().rev() {
            anchored.push(name);
        }
        Ok(anchored)
    }

    fn hold(&self, root: &Path) -> Result<Box<dyn RestoreHold>, RestoreFault> {
        let path = restore_lock_path(root)?;
        let file = open(&path, OpenMode::OpenOrCreate).map_err(|_| RestoreFault::Unreadable)?;
        file.lock().map_err(|_| RestoreFault::Unreadable)?;
        Ok(Box::new(RootHold { _file: file }))
    }

    fn mark_ready(&self, root: &Path) -> Result<(), RestoreFault> {
        let path = ready_path(root);
        let temp = root.join("accept-ready.partial");
        if temp.exists() {
            fs::remove_file(&temp).map_err(|_| RestoreFault::Unreadable)?;
        }
        if path.exists() {
            fs::remove_file(&path).map_err(|_| RestoreFault::Unreadable)?;
        }
        let mut file = open(&temp, OpenMode::CreateNew).map_err(|_| RestoreFault::Unreadable)?;
        let synced = file.write_all(b"ready\n").and_then(|_| file.sync_all());
        drop(file);
        if synced.is_err() {
            clear_ready_file(&temp);
            clear_ready_file(&path);
            return Err(RestoreFault::Unreadable);
        }
        if fs::rename(&temp, &path).is_err() {
            clear_ready_file(&temp);
            clear_ready_file(&path);
            return Err(RestoreFault::Unreadable);
        }
        Ok(())
    }

    fn ready(&self, root: &Path) -> Result<bool, RestoreFault> {
        let path = ready_path(root);
        if !path.is_file() {
            return Ok(false);
        }
        Ok(fs::read(&path).map_err(|_| RestoreFault::Unreadable)? == b"ready\n")
    }

    fn spoil(&self, path: &Path) -> Result<(), RestoreFault> {
        // This fallback destroys a session the storage opener may refuse:
        // mode is not 0600, or the Windows ACL is not the one `open` trusts.
        // Truncate with the process file API. On Unix, make the file writable
        // first so a read-only copy can still be emptied.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
        }
        let file = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(path)
            .map_err(|_| RestoreFault::Unreadable)?;
        file.sync_all().map_err(|_| RestoreFault::Unreadable)?;
        Ok(())
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

struct RootHold {
    _file: File,
}

impl RestoreHold for RootHold {}

/// Inside the quarantine, so a later tree cannot inherit a marker that
/// belonged to a directory already renamed away. Not part of the seal.
fn ready_path(root: &Path) -> PathBuf {
    root.join("accept-ready")
}

/// Beside the session file, so the publication names this copy and not an
/// earlier fetch of the same conversation.
fn publication_file(session: &Path) -> PathBuf {
    let mut name = session.as_os_str().to_owned();
    name.push(".publication");
    PathBuf::from(name)
}

/// `canonicalize` on Windows returns `\\?\C:\...`. The storage opener accepts
/// only a drive prefix, so the verbatim prefix is removed after the ancestor
/// is resolved.
fn plain_absolute(path: PathBuf) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path;
    };
    let Some(rest) = text.strip_prefix(r"\\?\") else {
        return path;
    };
    if let Some(unc) = rest.strip_prefix(r"UNC\") {
        return PathBuf::from(format!(r"\\{unc}"));
    }
    PathBuf::from(rest)
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
        self.armed = false;
        if let Some(parent) = self.quarantine.parent() {
            if sync_directory(parent).is_err() {
                if fs::remove_dir_all(&self.quarantine).is_err() {
                    return Err(RestoreFault::Retained);
                }
                return Err(RestoreFault::Unreadable);
            }
        }
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

fn clear_ready_file(path: &Path) {
    let _ = fs::remove_file(path);
    if path.is_file() {
        let _ = fs::OpenOptions::new().write(true).truncate(true).open(path);
    }
}

fn walk(
    root: &Path,
    directory: &Path,
    files: &mut Vec<SealedFile>,
    extras: &mut Vec<String>,
    helpers: &mut Vec<String>,
) -> Result<(), RestoreFault> {
    for entry in fs::read_dir(directory).map_err(|_| RestoreFault::Unreadable)? {
        let entry = entry.map_err(|_| RestoreFault::Unreadable)?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).map_err(|_| RestoreFault::Unreadable)?;
        if meta.file_type().is_symlink() {
            return Err(RestoreFault::Tamper);
        }
        if meta.is_dir() {
            walk(root, &path, files, extras, helpers)?;
            continue;
        }
        if !meta.is_file() {
            return Err(RestoreFault::Tamper);
        }
        let relative = relative_to(root, &path)?;
        if relative == "journal.txt" || relative == "accept-ready" {
            continue;
        }
        if session_sidecar(&relative).is_some() {
            helpers.push(relative);
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

fn sync_writable(path: &Path) -> Result<(), RestoreFault> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|_| RestoreFault::Unreadable)?;
    file.sync_all().map_err(|_| RestoreFault::Unreadable)?;
    Ok(())
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

    fn available(
        &self,
        conversation_id: &str,
        publication: &str,
    ) -> Result<bool, DecisionAuditError> {
        let _gate = self
            .gate
            .lock()
            .map_err(|_| DecisionAuditError::NotStored)?;
        let _file = exclusive(&self.directory).map_err(|_| DecisionAuditError::NotStored)?;
        if !self.path.is_file() {
            return Ok(false);
        }
        let text = fs::read_to_string(&self.path).map_err(|_| DecisionAuditError::NotStored)?;
        for line in text.lines().filter(|line| !line.is_empty()) {
            let value: Value =
                serde_json::from_str(line).map_err(|_| DecisionAuditError::NotStored)?;
            if value.get("after").and_then(Value::as_str) == Some("available")
                && value.get("decision").and_then(Value::as_str) == Some("effect")
                && value.get("attestations").and_then(Value::as_str) == Some(publication)
                && value
                    .get("target")
                    .and_then(Value::as_str)
                    .is_some_and(|target| {
                        crate::backup::application::fetch_names_conversation(
                            target,
                            conversation_id,
                        )
                    })
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn sealed(&self, target: &str, digest: &str) -> Result<bool, DecisionAuditError> {
        let _gate = self
            .gate
            .lock()
            .map_err(|_| DecisionAuditError::NotStored)?;
        let _file = exclusive(&self.directory).map_err(|_| DecisionAuditError::NotStored)?;
        if !self.path.is_file() {
            return Ok(false);
        }
        let text = fs::read_to_string(&self.path).map_err(|_| DecisionAuditError::NotStored)?;
        for line in text.lines().filter(|line| !line.is_empty()) {
            let value: Value =
                serde_json::from_str(line).map_err(|_| DecisionAuditError::NotStored)?;
            let field = |name: &str| value.get(name).and_then(Value::as_str).unwrap_or("");
            if value.get("target").and_then(Value::as_str) == Some(target)
                && crate::backup::application::effect_names_seal(
                    field("before"),
                    field("after"),
                    field("cause"),
                    field("decision"),
                    field("attestations"),
                    digest,
                )
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// Sixteen bytes from the operating system, written `pub-` plus lowercase hex.
pub struct OsPublicationIds;

impl PublicationIds for OsPublicationIds {
    fn publication_id(&self) -> Result<String, RestoreFault> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|_| RestoreFault::Unreadable)?;
        let mut name = String::from("pub-");
        for byte in bytes {
            name.push_str(&format!("{byte:02x}"));
        }
        Ok(name)
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
fn a_verbatim_windows_prefix_is_a_drive_path() {
    let plain = plain_absolute(PathBuf::from(r"\\?\C:\restore\quarantine"));
    assert_eq!(plain, PathBuf::from(r"C:\restore\quarantine"));
    let unc = plain_absolute(PathBuf::from(r"\\?\UNC\server\share"));
    assert_eq!(unc, PathBuf::from(r"\\server\share"));
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
    assert!(!first.sealed("quarantine", "digest-a").unwrap());
}

#[test]
fn a_sealed_audit_names_the_journal_digest() {
    struct Clock;
    impl ExportClock for Clock {
        fn unix_ms(&self) -> u64 {
            1
        }
    }

    let root = tempfile::tempdir().unwrap();
    let audit = FileRestoreAudit::open(root.path().join("audit"), Clock).unwrap();
    audit
        .append(&RestoreEvidence {
            target: "quarantine",
            before: "absent",
            after: "verified",
            cause: "sealed",
            initiator: "person",
            decision: "effect",
            policies: &[],
            attestations: "digest-a",
        })
        .unwrap();
    assert!(audit.sealed("quarantine", "digest-a").unwrap());
    assert!(!audit.sealed("quarantine", "digest-b").unwrap());
    assert!(!audit.sealed("other", "digest-a").unwrap());
}

#[test]
fn a_partial_readiness_file_is_not_the_marker() {
    let root = tempfile::tempdir().unwrap();
    let tree = root.path().join("quarantine");
    fs::create_dir(&tree).unwrap();
    fs::write(tree.join("accept-ready.partial"), b"ready\n").unwrap();
    assert!(!DirectoryQuarantine.ready(&tree).unwrap());
    DirectoryQuarantine.mark_ready(&tree).unwrap();
    assert!(DirectoryQuarantine.ready(&tree).unwrap());
    assert!(!tree.join("accept-ready.partial").exists());
    clear_ready_file(&tree.join("accept-ready"));
    assert!(!DirectoryQuarantine.ready(&tree).unwrap());
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
