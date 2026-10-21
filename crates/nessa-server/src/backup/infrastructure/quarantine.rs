//! Quarantine directories, sealed inventory files, and deferred session copies.
//!
//! The application decides what may be kept. This module reads JSON records,
//! writes the partial tree, and streams one oversized conversation.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use nessa_local_storage::{create_directory, open, publish_new, sync_directory, OpenMode};
use serde_json::{json, Value};

use sha2::Digest;

use crate::backup::application::{
    current_staging_id, CutMaterializer, DecisionAuditError, ExportClock, FetchProgress,
    LoadedTree, MacKey, PreparedCut, Promoted, PublicationIds, QuarantineLayout, RestoreAudit,
    RestoreEvidence, RestoreHold, SealFields, SealedInventoryReader, SessionCut,
    SessionSourceError, Staging, StagingIds, TreeId, VerificationFormat, VerificationLimits,
    VerificationSource,
};
use crate::backup::domain::{
    conversation_is_fenced, credential_is_fenced, encode_deferred, fresher_fences,
    leases_to_interrupt, metadata_omitted, normalized_root, paths_conflict_on,
    relative_path_is_safe, restore_lock_path, session_sidecar, share_grant_is_fenced,
    staging_id_is_usable, CredentialKind, DeferredEntry, DeferredLine, DeletionFence, DeletionLog,
    FenceKind, Fingerprint, LeaseMark, ManifestListing, MetadataFacts, MetadataRow, RestoreFault,
    SealedFile, StoreKind, VolumeCase, VolumeProbe, IDENTITY_BODY, MAX_NAME_BYTES, REPLAY_BODY,
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
    fn volume(&self, path: &Path) -> Result<VolumeProbe, RestoreFault> {
        probe_parent(path)
    }

    fn begin(&self, quarantine: &Path, live: &Path) -> Result<Box<dyn Staging>, RestoreFault> {
        distinct(self, quarantine, live)?;
        if symlink_exists(quarantine)? {
            return Err(RestoreFault::QuarantineOccupied);
        }
        let partial = claim_partial(self, quarantine, live)?;
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
            identity: tree_id(root)?,
        })
    }

    fn promote(
        &self,
        quarantine: &Path,
        live: &Path,
        identity: &TreeId,
    ) -> Result<Promoted, RestoreFault> {
        let opened = File::open(quarantine).map_err(|_| RestoreFault::Unreadable)?;
        if tree_id_of(&opened)? != *identity || tree_id(quarantine)? != *identity {
            return Err(RestoreFault::Tamper);
        }
        let from_parent = quarantine.parent().map(Path::to_path_buf);
        let to_parent = live.parent().map(Path::to_path_buf);
        // The rename itself refuses an existing destination, including an
        // empty directory created where a separate existence check would miss it.
        rename_new(quarantine, live, RestoreFault::LiveOccupied)?;
        // A stat failure is not a match. Leaving it as `?` would report the
        // promotion unread while a different directory still sits at live.
        let live_id = tree_id(live).ok();
        let opened_id = tree_id_of(&opened).ok();
        if live_id.as_ref() != Some(identity) || opened_id.as_ref() != Some(identity) {
            if !quarantine.exists() && live_id.as_ref() != Some(identity) {
                let _ = rename_new(live, quarantine, RestoreFault::Tamper);
            }
            return if live.exists() {
                Err(RestoreFault::Retained)
            } else {
                Err(RestoreFault::Tamper)
            };
        }
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
        // Durable bytes, then the name. A read-only open cannot flush on
        // Windows, so the writable handle is taken before the rename.
        // The rename refuses a destination that already exists.
        sync_writable(temp)?;
        rename_new(temp, final_path, RestoreFault::SessionDiffers)?;
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

    fn mark_ready(&self, root: &Path, staging_id: &str) -> Result<(), RestoreFault> {
        let body = marker_bytes(staging_id)?;
        publish_marker(root, &body, |directory| {
            sync_directory(directory).map_err(|_| RestoreFault::Unreadable)
        })
    }

    fn ready(&self, root: &Path) -> Result<Option<String>, RestoreFault> {
        read_marker_id(&ready_path(root))
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
        rename_new(
            &self.partial,
            &self.quarantine,
            RestoreFault::QuarantineOccupied,
        )?;
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

    fn discard(mut self: Box<Self>) -> Result<(), RestoreFault> {
        self.armed = false;
        remove_partial(&self.partial)
    }
}

impl Drop for PartialTree {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.partial);
        }
    }
}

fn marker_bytes(staging_id: &str) -> Result<Vec<u8>, RestoreFault> {
    if !staging_id_is_usable(staging_id) {
        return Err(RestoreFault::Unreadable);
    }
    let mut body = staging_id.as_bytes().to_vec();
    body.push(b'\n');
    Ok(body)
}

fn read_marker_id(path: &Path) -> Result<Option<String>, RestoreFault> {
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(path).map_err(|_| RestoreFault::Unreadable)?;
    let Some(text) = bytes.strip_suffix(b"\n") else {
        return Ok(None);
    };
    let Ok(id) = std::str::from_utf8(text) else {
        return Ok(None);
    };
    if staging_id_is_usable(id) {
        Ok(Some(id.to_owned()))
    } else {
        Ok(None)
    }
}

/// Remove a readiness file. `Retained` means it still names a staging id.
fn retire_marker(path: &Path) -> Result<(), RestoreFault> {
    retire_marker_with(
        path,
        |path| fs::remove_file(path),
        |path| {
            let file = fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(path)?;
            file.sync_all()
        },
    )
}

fn retire_marker_with(
    path: &Path,
    mut remove: impl FnMut(&Path) -> std::io::Result<()>,
    mut spoil: impl FnMut(&Path) -> std::io::Result<()>,
) -> Result<(), RestoreFault> {
    if !path.exists() {
        return Ok(());
    }
    let _ = remove(path);
    if !path.exists() {
        return Ok(());
    }
    let _ = spoil(path);
    match read_marker_id(path) {
        Ok(Some(_)) => Err(RestoreFault::Retained),
        _ => Ok(()),
    }
}

/// Rename `from` onto `to` only when `to` does not exist.
fn rename_new(from: &Path, to: &Path, occupied: RestoreFault) -> Result<(), RestoreFault> {
    match publish_new(from, to) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Err(occupied),
        Err(_) => Err(RestoreFault::Rename),
    }
}

fn remove_partial(path: &Path) -> Result<(), RestoreFault> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) if !path.exists() => Ok(()),
        Err(_) => Err(RestoreFault::Retained),
    }
}

/// Write the readiness marker, rename it into place, then sync `root`.
///
/// A failed directory sync clears the marker. A partial file is not readiness.
fn publish_marker(
    root: &Path,
    body: &[u8],
    mut sync_root: impl FnMut(&Path) -> Result<(), RestoreFault>,
) -> Result<(), RestoreFault> {
    let path = ready_path(root);
    let temp = root.join("accept-ready.partial");
    if temp.exists() {
        fs::remove_file(&temp).map_err(|_| RestoreFault::Unreadable)?;
    }
    if path.exists() {
        fs::remove_file(&path).map_err(|_| RestoreFault::Unreadable)?;
    }
    let mut file = open(&temp, OpenMode::CreateNew).map_err(|_| RestoreFault::Unreadable)?;
    let synced = file.write_all(body).and_then(|_| file.sync_all());
    drop(file);
    if synced.is_err() {
        return retire_both(&temp, &path, RestoreFault::Unreadable);
    }
    if rename_new(&temp, &path, RestoreFault::Unreadable).is_err() {
        return retire_both(&temp, &path, RestoreFault::Unreadable);
    }
    if sync_root(root).is_err() {
        return retire_both(&temp, &path, RestoreFault::Unreadable);
    }
    Ok(())
}

fn retire_both(temp: &Path, path: &Path, cleared: RestoreFault) -> Result<(), RestoreFault> {
    let temp_gone = retire_marker(temp);
    let path_gone = retire_marker(path);
    if temp_gone.is_err() || path_gone.is_err() {
        return Err(RestoreFault::Retained);
    }
    Err(cleared)
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

static PARTIAL_GATE: Mutex<()> = Mutex::new(());
static ATTEMPT: AtomicU64 = AtomicU64::new(1);

fn distinct(layout: &DirectoryQuarantine, left: &Path, right: &Path) -> Result<(), RestoreFault> {
    paths_conflict_on(left, right, layout.volume(left)?, layout.volume(right)?)
}

/// Create a partial directory this attempt owns. An existing name is left alone.
fn claim_partial(
    layout: &DirectoryQuarantine,
    quarantine: &Path,
    live: &Path,
) -> Result<PathBuf, RestoreFault> {
    let _gate = PARTIAL_GATE.lock().map_err(|_| RestoreFault::Unreadable)?;
    claim_partial_locked(layout, quarantine, live)
}

fn claim_partial_locked(
    layout: &DirectoryQuarantine,
    quarantine: &Path,
    live: &Path,
) -> Result<PathBuf, RestoreFault> {
    for _ in 0..32 {
        let partial = partial_path(quarantine)?;
        if distinct(layout, &partial, live).is_err() {
            continue;
        }
        match mkdir_exclusive(&partial) {
            Ok(()) => return Ok(partial),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(RestoreFault::Unreadable),
        }
    }
    Err(RestoreFault::Unreadable)
}

fn partial_path(quarantine: &Path) -> Result<PathBuf, RestoreFault> {
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

fn tree_id(path: &Path) -> Result<TreeId, RestoreFault> {
    #[cfg(windows)]
    refuse_reparse(path)?;
    let file = File::open(path).map_err(|_| RestoreFault::Unreadable)?;
    tree_id_of(&file)
}

fn tree_id_of(file: &File) -> Result<TreeId, RestoreFault> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = file.metadata().map_err(|_| RestoreFault::Unreadable)?;
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&meta.ino().to_ne_bytes());
        Ok(TreeId {
            volume: meta.dev(),
            file: bytes,
        })
    }
    #[cfg(windows)]
    {
        let (volume, index) = windows_file_id(file)?;
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&index.to_ne_bytes());
        Ok(TreeId {
            volume,
            file: bytes,
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Ok(TreeId {
            volume: 0,
            file: [0; 16],
        })
    }
}

fn probe_parent(path: &Path) -> Result<VolumeProbe, RestoreFault> {
    let start = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => path,
    };
    let ancestor = existing_ancestor(start)?;
    Ok(VolumeProbe {
        device: device_of(&ancestor)?,
        case: case_of(&ancestor)?,
    })
}

fn existing_ancestor(path: &Path) -> Result<PathBuf, RestoreFault> {
    let mut current = path.to_path_buf();
    loop {
        match fs::metadata(&current) {
            Ok(meta) if meta.is_dir() => return Ok(current),
            Ok(_) => return Err(RestoreFault::Unreadable),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !current.pop() {
                    return Err(RestoreFault::Paths);
                }
            }
            Err(_) => return Err(RestoreFault::Unreadable),
        }
    }
}

fn device_of(dir: &Path) -> Result<u64, RestoreFault> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = fs::metadata(dir).map_err(|_| RestoreFault::Unreadable)?;
        Ok(meta.dev())
    }
    #[cfg(windows)]
    {
        let file = File::open(dir).map_err(|_| RestoreFault::Unreadable)?;
        Ok(windows_file_id(&file)?.0)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = dir;
        Ok(0)
    }
}

/// Volume serial and file index from the open handle.
///
/// `MetadataExt::file_index` is the unstable `windows_by_handle` feature, so
/// this reads `BY_HANDLE_FILE_INFORMATION` directly.
#[cfg(windows)]
fn windows_file_id(file: &File) -> Result<(u64, u64), RestoreFault> {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    struct ByHandleFileInformation {
        attributes: u32,
        creation_low: u32,
        creation_high: u32,
        access_low: u32,
        access_high: u32,
        write_low: u32,
        write_high: u32,
        volume_serial: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetFileInformationByHandle(
            file: *mut std::ffi::c_void,
            information: *mut ByHandleFileInformation,
        ) -> i32;
    }
    let mut info = unsafe { std::mem::zeroed::<ByHandleFileInformation>() };
    // The handle is the still-open file. The call writes one information struct.
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
    if ok == 0 {
        return Err(RestoreFault::Unreadable);
    }
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    if info.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(RestoreFault::Tamper);
    }
    let index = (u64::from(info.index_high) << 32) | u64::from(info.index_low);
    Ok((u64::from(info.volume_serial), index))
}

#[cfg(windows)]
fn refuse_reparse(path: &Path) -> Result<(), RestoreFault> {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    let meta = fs::symlink_metadata(path).map_err(|_| RestoreFault::Unreadable)?;
    if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        Err(RestoreFault::Tamper)
    } else {
        Ok(())
    }
}

fn case_of(dir: &Path) -> Result<VolumeCase, RestoreFault> {
    #[cfg(target_os = "linux")]
    {
        linux_case(dir)
    }
    #[cfg(target_os = "macos")]
    {
        macos_case(dir)
    }
    #[cfg(windows)]
    {
        let _ = dir;
        Ok(VolumeCase::Unicode)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = dir;
        Ok(VolumeCase::Bytes)
    }
}

/// Case rule for a reported filesystem. `casefold` is the ext4 directory flag.
fn case_for_reported_fs(fstype: &str, casefold: bool) -> VolumeCase {
    match fstype {
        "ext2" | "ext3" | "ext4" => {
            if casefold {
                VolumeCase::Unicode
            } else {
                VolumeCase::Bytes
            }
        }
        "vfat" | "msdos" | "exfat" | "ntfs" | "ntfs3" | "ntfs-3g" | "fuse.ntfs-3g" | "cifs"
        | "smb" | "smb3" | "iso9660" | "udf" | "fuseblk" => VolumeCase::Unicode,
        _ => VolumeCase::Bytes,
    }
}

#[cfg(target_os = "linux")]
fn linux_case(dir: &Path) -> Result<VolumeCase, RestoreFault> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let name = CString::new(dir.as_os_str().as_bytes()).map_err(|_| RestoreFault::Unreadable)?;
    let mut stat = unsafe { std::mem::zeroed::<libc::statfs>() };
    if unsafe { libc::statfs(name.as_ptr(), &mut stat) } != 0 {
        return Err(RestoreFault::Unreadable);
    }
    const EXT: i64 = 0xEF53;
    const VFAT: i64 = 0x4d44;
    const EXFAT: i64 = 0x2011_BAB0;
    const NTFS: i64 = 0x5346_544e;
    const CIFS: i64 = 0xFF53_4D42;
    const ISO: i64 = 0x9660;
    const UDF: i64 = 0x1501_3346;
    const FUSE: i64 = 0x6573_5546;
    let magic = stat.f_type as i64;
    let folding = matches!(magic, VFAT | EXFAT | NTFS | CIFS | ISO | UDF);
    if folding {
        return Ok(VolumeCase::Unicode);
    }
    if magic == EXT {
        return Ok(match ext_casefold(dir) {
            Some(false) => VolumeCase::Bytes,
            Some(true) | None => VolumeCase::Unicode,
        });
    }
    if magic == FUSE {
        let kind = mount_fstype(dir).unwrap_or_default();
        if kind.is_empty() {
            return Ok(VolumeCase::Unicode);
        }
        return Ok(case_for_reported_fs(&kind, false));
    }
    Ok(VolumeCase::Bytes)
}

#[cfg(target_os = "linux")]
/// `Some(false)` is a readable ext directory that is not casefolded.
/// `None` means the flag could not be read, so the caller folds.
fn ext_casefold(dir: &Path) -> Option<bool> {
    use std::os::fd::AsRawFd;
    const FS_IOC_GETFLAGS: libc::Ioctl = 0x8008_6601;
    const FS_CASEFOLD_FL: libc::c_long = 0x4000_0000;
    let file = File::open(dir).ok()?;
    let mut flags: libc::c_long = 0;
    let read = unsafe { libc::ioctl(file.as_raw_fd(), FS_IOC_GETFLAGS, &mut flags) };
    if read != 0 {
        return None;
    }
    Some(flags & FS_CASEFOLD_FL != 0)
}

#[cfg(target_os = "linux")]
fn mount_fstype(dir: &Path) -> Option<String> {
    let text = fs::read_to_string("/proc/self/mountinfo").ok()?;
    let target = dir.to_str()?;
    let mut best: Option<(usize, String)> = None;
    for line in text.lines() {
        let Some((pre, post)) = line.split_once(" - ") else {
            continue;
        };
        let Some(mount) = pre.split_whitespace().nth(4) else {
            continue;
        };
        let Some(fstype) = post.split_whitespace().next() else {
            continue;
        };
        let matches = target == mount || mount == "/" || target.starts_with(&format!("{mount}/"));
        if matches && best.as_ref().is_none_or(|(len, _)| mount.len() > *len) {
            best = Some((mount.len(), fstype.to_owned()));
        }
    }
    best.map(|(_, fstype)| fstype)
}

#[cfg(target_os = "macos")]
fn macos_case(dir: &Path) -> Result<VolumeCase, RestoreFault> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let name = CString::new(dir.as_os_str().as_bytes()).map_err(|_| RestoreFault::Unreadable)?;
    let sensitive = unsafe { libc::pathconf(name.as_ptr(), libc::_PC_CASE_SENSITIVE) };
    if sensitive == 1 {
        Ok(VolumeCase::Bytes)
    } else {
        Ok(VolumeCase::UnicodeDecomposed)
    }
}

fn mkdir_exclusive(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let name = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "nul"))?;
        if unsafe { libc::mkdir(name.as_ptr(), 0o700) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(not(unix))]
    {
        fs::create_dir(path)
    }
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

    fn sealed(&self, target: &str, digest: &str) -> Result<Option<String>, DecisionAuditError> {
        let _gate = self
            .gate
            .lock()
            .map_err(|_| DecisionAuditError::NotStored)?;
        let _file = exclusive(&self.directory).map_err(|_| DecisionAuditError::NotStored)?;
        if !self.path.is_file() {
            return Ok(None);
        }
        let text = fs::read_to_string(&self.path).map_err(|_| DecisionAuditError::NotStored)?;
        let mut lines = Vec::new();
        for line in text.lines().filter(|line| !line.is_empty()) {
            let value: Value =
                serde_json::from_str(line).map_err(|_| DecisionAuditError::NotStored)?;
            let field = |name: &str| value.get(name).and_then(Value::as_str).unwrap_or("");
            lines.push((
                field("target").to_owned(),
                field("before").to_owned(),
                field("after").to_owned(),
                field("cause").to_owned(),
                field("decision").to_owned(),
                field("attestations").to_owned(),
            ));
        }
        Ok(current_staging_id(
            lines.iter().map(|line| SealFields {
                target: &line.0,
                before: &line.1,
                after: &line.2,
                cause: &line.3,
                decision: &line.4,
                attestations: &line.5,
            }),
            target,
            digest,
        ))
    }
}

/// Sixteen bytes from the operating system, written `pub-` plus lowercase hex.
pub struct OsPublicationIds;

impl PublicationIds for OsPublicationIds {
    fn publication_id(&self) -> Result<String, RestoreFault> {
        random_id("pub-")
    }
}

/// Sixteen bytes from the operating system, written `stage-` plus lowercase hex.
pub struct OsStagingIds;

impl StagingIds for OsStagingIds {
    fn staging_id(&self) -> Result<String, RestoreFault> {
        random_id("stage-")
    }
}

fn random_id(prefix: &str) -> Result<String, RestoreFault> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| RestoreFault::Unreadable)?;
    let mut name = String::from(prefix);
    for byte in bytes {
        name.push_str(&format!("{byte:02x}"));
    }
    Ok(name)
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
fn a_preexisting_partial_directory_is_not_deleted() {
    let root = tempfile::tempdir().unwrap();
    let quarantine = root.path().join("q");
    let live = root.path().join("live");
    let _gate = PARTIAL_GATE.lock().unwrap();
    let attempt = ATTEMPT.load(Ordering::Relaxed);
    let planted = root
        .path()
        .join(format!("q.partial-{}-{attempt}", std::process::id()));
    fs::create_dir_all(&planted).unwrap();
    fs::write(planted.join("owned"), b"keep").unwrap();
    let claimed = claim_partial_locked(&DirectoryQuarantine, &quarantine, &live).unwrap();
    assert_ne!(claimed, planted);
    assert_eq!(fs::read(planted.join("owned")).unwrap(), b"keep");
    assert!(claimed.is_dir());
    let _ = quarantine;
}

#[test]
fn case_for_a_reported_filesystem_follows_the_volume() {
    assert_eq!(case_for_reported_fs("vfat", false), VolumeCase::Unicode);
    assert_eq!(case_for_reported_fs("exfat", false), VolumeCase::Unicode);
    assert_eq!(case_for_reported_fs("ntfs", false), VolumeCase::Unicode);
    assert_eq!(case_for_reported_fs("ntfs3", false), VolumeCase::Unicode);
    assert_eq!(case_for_reported_fs("ext4", false), VolumeCase::Bytes);
    assert_eq!(case_for_reported_fs("ext4", true), VolumeCase::Unicode);
    assert_eq!(case_for_reported_fs("tmpfs", false), VolumeCase::Bytes);
    let probed = probe_parent(&std::env::temp_dir()).unwrap();
    #[cfg(target_os = "linux")]
    assert_eq!(probed.case, VolumeCase::Bytes);
    #[cfg(target_os = "macos")]
    assert_eq!(probed.case, VolumeCase::UnicodeDecomposed);
    #[cfg(windows)]
    assert_eq!(probed.case, VolumeCase::Unicode);
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
    assert!(first.sealed("quarantine", "digest-a").unwrap().is_none());
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
    let attestation = crate::backup::domain::seal_attestation("digest-a", "stage-1");
    audit
        .append(&RestoreEvidence {
            target: "quarantine",
            before: "absent",
            after: "verified",
            cause: "sealed",
            initiator: "person",
            decision: "effect",
            policies: &[],
            attestations: &attestation,
        })
        .unwrap();
    assert_eq!(
        audit.sealed("quarantine", "digest-a").unwrap().as_deref(),
        Some("stage-1")
    );
    assert!(audit.sealed("quarantine", "digest-b").unwrap().is_none());
    assert!(audit.sealed("other", "digest-a").unwrap().is_none());
    audit
        .append(&RestoreEvidence {
            target: "quarantine",
            before: "verified",
            after: "spent",
            cause: "spent",
            initiator: "person",
            decision: "effect",
            policies: &[],
            attestations: "stage-1",
        })
        .unwrap();
    assert!(audit.sealed("quarantine", "digest-a").unwrap().is_none());
}

#[test]
fn a_partial_readiness_file_is_not_the_marker() {
    let root = tempfile::tempdir().unwrap();
    let tree = root.path().join("quarantine");
    fs::create_dir(&tree).unwrap();
    fs::write(tree.join("accept-ready.partial"), b"ready\n").unwrap();
    assert!(DirectoryQuarantine.ready(&tree).unwrap().is_none());
    DirectoryQuarantine.mark_ready(&tree, "stage-1").unwrap();
    assert_eq!(
        DirectoryQuarantine.ready(&tree).unwrap().as_deref(),
        Some("stage-1")
    );
    assert!(!tree.join("accept-ready.partial").exists());
    retire_marker(&tree.join("accept-ready")).unwrap();
    assert!(DirectoryQuarantine.ready(&tree).unwrap().is_none());
}

#[test]
fn a_readiness_directory_sync_failure_clears_the_marker() {
    let root = tempfile::tempdir().unwrap();
    let tree = root.path().join("quarantine");
    fs::create_dir(&tree).unwrap();
    let error = publish_marker(&tree, b"stage-1\n", |_| Err(RestoreFault::Unreadable)).unwrap_err();
    assert_eq!(error, RestoreFault::Unreadable);
    assert!(!tree.join("accept-ready").exists());
    assert!(!tree.join("accept-ready.partial").exists());
    assert!(DirectoryQuarantine.ready(&tree).unwrap().is_none());
}

#[test]
fn a_marker_that_cannot_be_cleared_is_retained() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("accept-ready");
    fs::write(&marker, b"stage-1\n").unwrap();
    let error = retire_marker_with(
        &marker,
        |_| Err(std::io::Error::other("remove")),
        |_| Err(std::io::Error::other("spoil")),
    )
    .unwrap_err();
    assert_eq!(error, RestoreFault::Retained);
    assert_eq!(fs::read(&marker).unwrap(), b"stage-1\n");
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
