//! Quarantine directories, sealed inventory files, and deferred session copies.
//!
//! The application decides what may be kept. This module reads JSON records,
//! writes the partial tree, and streams one oversized conversation.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

#[cfg(any(test, windows))]
use nessa_local_storage::create_directory_exclusive;
#[cfg(windows)]
use nessa_local_storage::create_no_writers;
use nessa_local_storage::{create_directory, open, publish_new, sync_directory, OpenMode};
use serde_json::{json, Value};

use sha2::Digest;

use crate::backup::application::{
    current_staging_id, AnchoredRoot, CutMaterializer, DecisionAuditError, ExportClock,
    FetchProgress, LoadedTree, MacKey, PreparedCut, Promoted, PublicationIds, QuarantineLayout,
    RestoreAudit, RestoreEvidence, RestoreHold, SealFields, SealedInventoryReader, SessionCut,
    SessionSourceError, Staging, StagingIds, TreeId, VerificationFormat, VerificationLimits,
    VerificationSource,
};
use crate::backup::domain::{
    conversation_is_fenced, credential_is_fenced, encode_deferred, fresher_fences,
    leases_to_interrupt, metadata_omitted, normalized_root, paths_conflict_on,
    relative_path_is_safe, session_sidecar, share_grant_is_fenced, staging_id_is_usable,
    CredentialKind, DeferredEntry, DeferredLine, DeletionFence, DeletionLog, FenceKind,
    Fingerprint, LeaseMark, ManifestListing, MetadataFacts, MetadataRow, RestoreFault, SealedFile,
    StoreKind, VolumeCase, VolumeProbe, IDENTITY_BODY, MAX_NAME_BYTES, REPLAY_BODY,
};

use super::log::{authenticate_inventory, exclusive, persist};
#[cfg(unix)]
use super::snapshot::inspect_open_file;
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
        if credential.kind != kind || credential.id != identity {
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

    fn begin(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
    ) -> Result<Box<dyn Staging>, RestoreFault> {
        paths_conflict_on(
            quarantine.path(),
            live.path(),
            quarantine.probe(),
            live.probe(),
        )?;
        let (parent, name) = parent_of_root(quarantine)?;
        if name_exists_at(&parent, &name)? {
            return Err(RestoreFault::QuarantineOccupied);
        }
        let _gate = PARTIAL_GATE.lock().map_err(|_| RestoreFault::Unreadable)?;
        let (directory, partial_name) = claim_partial_locked(
            &parent,
            &name,
            live.path(),
            quarantine.probe(),
            live.probe(),
        )?;
        Ok(Box::new(PartialTree {
            directory,
            parent,
            partial_name,
            quarantine_name: name,
            armed: true,
        }))
    }

    fn load(&self, root: &AnchoredRoot) -> Result<LoadedTree, RestoreFault> {
        let directory = open_root_dir(root)?;
        if kind_at(&directory, OsStr::new("journal.txt"))? != EntryKind::File {
            return Err(RestoreFault::Unreadable);
        }
        let journal = read_file_at(&directory, OsStr::new("journal.txt"))?;
        let mut files = Vec::new();
        let mut extras = Vec::new();
        let mut helpers = Vec::new();
        walk_at(&directory, "", &mut files, &mut extras, &mut helpers)?;
        Ok(LoadedTree {
            journal,
            files,
            extras,
            helpers,
            identity: tree_id_of(&directory)?,
            content: content_of(&directory)?,
        })
    }

    fn promote(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
        loaded: &LoadedTree,
    ) -> Result<Promoted, RestoreFault> {
        let (source_parent, source_name) = parent_of_root(quarantine)?;
        let opened = open_dir_at(&source_parent, &source_name)?;
        if tree_id_of(&opened)? != loaded.identity {
            return Err(RestoreFault::Tamper);
        }
        let on_disk = read_file_at(&opened, OsStr::new("journal.txt"))?;
        if on_disk != loaded.journal || content_of(&opened)? != loaded.content {
            return Err(RestoreFault::Tamper);
        }
        // The live tree is a directory this attempt creates under the parent
        // this anchor holds, and writes from those sealed bytes. The identity
        // is that directory's open. An open descriptor on the quarantine file
        // keeps the old inode.
        let (live_parent, live_name) = parent_of_root(live)?;
        let SealedDir {
            name: sealed_name,
            file: created,
            identity: created_id,
            ..
        } = sealed_beside(&live_parent, &live_name)?;
        // Copied bytes stay on the handles that wrote them. On Unix those
        // inodes have no names yet, so a writer cannot open them. On Windows
        // the names exist and the handles do not share write access.
        let mut held = Vec::new();
        let copied = match copy_at(&opened, &created, &mut held) {
            Ok(token) => token,
            Err(error) => {
                drop(held);
                let _ = remove_opened(&created, &created_id);
                return Err(error);
            }
        };
        let mut journal = match create_unpublished(
            &created,
            OsStr::new("journal.txt"),
            "journal.txt",
            false,
            RestoreFault::Tamper,
        ) {
            Ok(file) => file,
            Err(error) => {
                drop(held);
                let _ = remove_opened(&created, &created_id);
                return Err(error);
            }
        };
        if journal.store(loaded.journal.as_bytes()).is_err() || journal.sync_file().is_err() {
            drop(held);
            drop(journal);
            let _ = remove_opened(&created, &created_id);
            return Err(RestoreFault::Unreadable);
        }
        if copied != loaded.content || tree_id_of(&opened)? != loaded.identity {
            drop(held);
            drop(journal);
            let _ = remove_opened(&created, &created_id);
            return Err(RestoreFault::Tamper);
        }
        if sync_tree_at(&created).is_err() {
            drop(held);
            drop(journal);
            let _ = remove_opened(&created, &created_id);
            return Err(RestoreFault::Unreadable);
        }
        if let Err(error) =
            held_still_sealed(&created, &created_id, loaded, &mut held, &mut journal)
        {
            drop(held);
            drop(journal);
            if tree_id_of(&created).ok().as_ref() == Some(&created_id) {
                let _ = remove_opened(&created, &created_id);
            }
            return Err(error);
        }
        // The link is the publication of each inode. The directory rename
        // then makes those names live. A descriptor opened on a predictable
        // copy name is not one of these inodes. The handles stay open across
        // that rename so the bytes cannot be replaced first.
        if let Err(error) = publish_held(&mut held, &mut journal) {
            drop(held);
            drop(journal);
            let _ = remove_opened(&created, &created_id);
            return Err(error);
        }
        let published = created_id;
        // The rename itself refuses an existing destination, including an
        // empty directory created where a separate existence check would miss it.
        if let Err(error) = rename_exclusive_at(
            &live_parent,
            &sealed_name,
            &live_parent,
            &live_name,
            RestoreFault::LiveOccupied,
        ) {
            drop(held);
            drop(journal);
            let _ = remove_opened(&created, &published);
            return Err(error);
        }
        let live_id = open_dir_at(&live_parent, &live_name)
            .ok()
            .and_then(|file| tree_id_of(&file).ok());
        let opened_id = tree_id_of(&created).ok();
        if live_id.as_ref() != Some(&published) || opened_id.as_ref() != Some(&published) {
            let sealed_gone = !name_exists_at(&live_parent, &sealed_name).unwrap_or(true);
            if sealed_gone && live_id.as_ref() != Some(&published) {
                let _ = rename_exclusive_at(
                    &live_parent,
                    &live_name,
                    &live_parent,
                    &sealed_name,
                    RestoreFault::Tamper,
                );
            }
            return if name_exists_at(&live_parent, &live_name).unwrap_or(true) {
                Err(RestoreFault::Retained)
            } else {
                Err(RestoreFault::Tamper)
            };
        }
        // The copy is live. Retire the source the open handle still names,
        // even when the original path is gone.
        retire_opened(&opened, &loaded.identity)?;
        // The rename already made the tree live. A parent that cannot be
        // synced does not put it back, and does not make the promotion a
        // refusal.
        let mut durable = true;
        if sync_dir(&live_parent).is_err() {
            durable = false;
        }
        if sync_dir(&source_parent).is_err() {
            durable = false;
        }
        Ok(Promoted { durable })
    }

    fn remove_tree(&self, path: &Path) -> Result<(), RestoreFault> {
        if path.exists() {
            fs::remove_dir_all(path).map_err(|_| RestoreFault::Unreadable)?;
        }
        Ok(())
    }

    fn remove_known(&self, path: &Path, identity: &TreeId) -> Result<(), RestoreFault> {
        remove_same(path, identity)
    }

    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        fs::read(path).map_err(|_| RestoreFault::Unreadable)
    }

    fn session_temp(&self, root: &AnchoredRoot, relative: &str) -> Result<PathBuf, RestoreFault> {
        // Parents are created beneath the held root. The partial file is not:
        // the source creates that one name.
        let directory = directory_under(&open_root_dir(root)?, relative)?;
        let leaf = relative_leaf(relative)?;
        let mut temp_name = leaf.to_os_string();
        temp_name.push(".partial");
        Ok(path_of_handle(&directory)?.join(temp_name))
    }

    fn publish_verified(
        &self,
        root: &AnchoredRoot,
        temp: &Path,
        relative: &str,
        conversation_id: &str,
        digest: &Fingerprint,
    ) -> Result<(), RestoreFault> {
        let directory = directory_for(root, relative)?;
        let leaf = relative_leaf(relative)?;
        let temp_name = temp.file_name().ok_or(RestoreFault::Tamper)?;
        // The copy has no directory entry until the session name is linked.
        // On Windows the session name is created here and held without write
        // share, so another open cannot change it before this handle closes.
        let mut input = open_file_at(&directory, temp_name, OpenMode::Read)?;
        let mut output = create_unpublished(
            &directory,
            &leaf,
            relative,
            true,
            RestoreFault::SessionDiffers,
        )?;
        let mut buffer = [0u8; 8192];
        loop {
            let count = match input.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => count,
                Err(_) => return Err(RestoreFault::Unreadable),
            };
            if output.store(&buffer[..count]).is_err() {
                return Err(RestoreFault::Unreadable);
            }
        }
        if output.sync_file().is_err() {
            return Err(RestoreFault::Unreadable);
        }
        let verified = unpublished_matches(&mut output, conversation_id, digest)?;
        // The name is linked only after the held bytes matched.
        if !verified {
            return Err(RestoreFault::SourceChanged);
        }
        output.publish_in(RestoreFault::SessionDiffers)?;
        if sync_dir(&directory).is_err() {
            let _ = unlink_at(&directory, &leaf);
            return Err(RestoreFault::Unreadable);
        }
        // The temp inode is not the published file. Removing it does not
        // change the bytes that matched `digest`.
        let _ = self.remove_file(temp);
        Ok(())
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
        match fs::symlink_metadata(path) {
            Ok(_) => fs::remove_file(path).map_err(|_| RestoreFault::Unreadable),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(RestoreFault::Unreadable),
        }
    }

    fn exists(&self, path: &Path) -> Result<bool, RestoreFault> {
        symlink_exists(path)
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, RestoreFault> {
        path.canonicalize().map_err(|_| RestoreFault::Unreadable)
    }

    fn anchor(&self, path: &Path) -> Result<AnchoredRoot, RestoreFault> {
        let normal = normalized_root(path)?;
        let mut prefix = normal.clone();
        let mut missing = Vec::new();
        while !prefix.exists() {
            let Some(name) = prefix.file_name().map(|name| name.to_os_string()) else {
                return Err(RestoreFault::Paths);
            };
            missing.push(name);
            if !prefix.pop() {
                return Err(RestoreFault::Paths);
            }
        }
        let (held, tail) = if missing.is_empty() {
            let name = prefix
                .file_name()
                .ok_or(RestoreFault::Paths)?
                .to_os_string();
            if !prefix.pop() {
                return Err(RestoreFault::Paths);
            }
            (prefix, vec![name])
        } else {
            (prefix, missing.into_iter().rev().collect())
        };
        let canonical = plain_absolute(held.canonicalize().map_err(|_| RestoreFault::Unreadable)?);
        let ancestor = open_ancestor(&canonical)?;
        let mut spelling = path_of_handle(&ancestor)?;
        for name in &tail {
            spelling.push(name);
        }
        let probe = probe_of(&ancestor)?;
        Ok(AnchoredRoot::bind(ancestor, tail, spelling, probe))
    }

    fn hold(&self, root: &AnchoredRoot) -> Result<Box<dyn RestoreHold>, RestoreFault> {
        let (parent, name) = parent_of_root(root)?;
        let mut lock_name = name;
        lock_name.push(".restore-lock");
        let file = open_file_at(&parent, &lock_name, OpenMode::OpenOrCreate)?;
        file.lock().map_err(|_| RestoreFault::Unreadable)?;
        Ok(Box::new(RootHold {
            _file: file,
            _parent: parent,
        }))
    }

    fn mark_ready(&self, root: &AnchoredRoot, staging_id: &str) -> Result<(), RestoreFault> {
        let body = marker_bytes(staging_id)?;
        let directory = open_root_dir(root)?;
        publish_marker_at(&directory, &body)
    }

    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        let directory = open_root_dir(root)?;
        match kind_at(&directory, OsStr::new("accept-ready"))? {
            EntryKind::File => {
                read_marker_bytes(&read_bytes_at(&directory, OsStr::new("accept-ready"))?)
            }
            _ => Ok(None),
        }
    }

    fn retire_ready(&self, root: &AnchoredRoot) -> Result<(), RestoreFault> {
        let directory = open_root_dir(root)?;
        let name = OsStr::new("accept-ready");
        if kind_at(&directory, name)? == EntryKind::Missing {
            return Ok(());
        }
        let _ = unlink_at(&directory, name);
        if kind_at(&directory, name)? == EntryKind::Missing {
            return Ok(());
        }
        if let Ok(file) = open_file_at(&directory, name, OpenMode::ReadWrite) {
            let _ = file.set_len(0);
            let _ = file.sync_all();
        }
        match read_bytes_at(&directory, name) {
            Ok(bytes) => match read_marker_bytes(&bytes)? {
                Some(_) => Err(RestoreFault::Retained),
                None => Ok(()),
            },
            Err(_) => Err(RestoreFault::Retained),
        }
    }

    fn occupied(&self, root: &AnchoredRoot) -> Result<bool, RestoreFault> {
        let (parent, name) = parent_of_root(root)?;
        name_exists_at(&parent, &name)
    }

    fn read_relative(&self, root: &AnchoredRoot, relative: &str) -> Result<Vec<u8>, RestoreFault> {
        let directory = directory_for(root, relative)?;
        let leaf = relative_leaf(relative)?;
        read_bytes_at(&directory, &leaf)
    }

    fn exists_relative(&self, root: &AnchoredRoot, relative: &str) -> Result<bool, RestoreFault> {
        if !relative_path_is_safe(relative) {
            return Err(RestoreFault::Tamper);
        }
        let mut directory = open_root_dir(root)?;
        let parts: Vec<_> = Path::new(relative).components().collect();
        for (index, component) in parts.iter().enumerate() {
            let std::path::Component::Normal(name) = component else {
                return Err(RestoreFault::Tamper);
            };
            let last = index + 1 == parts.len();
            match kind_at(&directory, name)? {
                EntryKind::Missing => return Ok(false),
                EntryKind::Directory if !last => directory = open_dir_at(&directory, name)?,
                EntryKind::Directory | EntryKind::File | EntryKind::Symlink | EntryKind::Other => {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn note_publication_relative(
        &self,
        root: &AnchoredRoot,
        relative: &str,
        publication: &str,
    ) -> Result<(), RestoreFault> {
        if publication.is_empty() || publication.bytes().any(|byte| byte <= b' ') {
            return Err(RestoreFault::Unreadable);
        }
        let directory = directory_for(root, relative)?;
        let mut name = relative_leaf(relative)?;
        name.push(".publication");
        if name_exists_at(&directory, &name)? {
            unlink_at(&directory, &name)?;
        }
        let mut file = open_file_at(&directory, &name, OpenMode::CreateNew)?;
        file.write_all(publication.as_bytes())
            .map_err(|_| RestoreFault::Unreadable)?;
        file.write_all(b"\n")
            .map_err(|_| RestoreFault::Unreadable)?;
        file.sync_all().map_err(|_| RestoreFault::Unreadable)?;
        sync_dir(&directory)?;
        Ok(())
    }

    fn publication_relative(
        &self,
        root: &AnchoredRoot,
        relative: &str,
    ) -> Result<Option<String>, RestoreFault> {
        let directory = directory_for(root, relative)?;
        let mut name = relative_leaf(relative)?;
        name.push(".publication");
        if kind_at(&directory, &name)? != EntryKind::File {
            return Ok(None);
        }
        let bytes = read_bytes_at(&directory, &name)?;
        let text = String::from_utf8(bytes).map_err(|_| RestoreFault::Tamper)?;
        let id = text.trim_end_matches(['\n', '\r']);
        if id.is_empty() || id.bytes().any(|byte| byte <= b' ') {
            return Err(RestoreFault::Tamper);
        }
        Ok(Some(id.to_owned()))
    }

    fn remove_publication_relative(
        &self,
        root: &AnchoredRoot,
        relative: &str,
    ) -> Result<(), RestoreFault> {
        let directory = directory_for(root, relative)?;
        let mut name = relative_leaf(relative)?;
        name.push(".publication");
        match kind_at(&directory, &name)? {
            EntryKind::Missing => Ok(()),
            EntryKind::File | EntryKind::Symlink => unlink_at(&directory, &name),
            EntryKind::Directory | EntryKind::Other => Err(RestoreFault::Unreadable),
        }
    }

    fn session_cut_relative(
        &self,
        root: &AnchoredRoot,
        relative: &str,
        conversation_id: &str,
    ) -> Result<SessionCut, RestoreFault> {
        let directory = directory_for(root, relative)?;
        let leaf = relative_leaf(relative)?;
        let file = open_file_at(&directory, &leaf, OpenMode::Read)?;
        let path = path_of_handle(&file)?;
        let cut = self.session_cut(&path, conversation_id)?;
        drop(file);
        Ok(cut)
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
    _parent: File,
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
    directory: File,
    parent: File,
    partial_name: OsString,
    quarantine_name: OsString,
    armed: bool,
}

impl Staging for PartialTree {
    fn write(&mut self, relative: &str, bytes: &[u8]) -> Result<(), RestoreFault> {
        if !relative_path_is_safe(relative) {
            return Err(RestoreFault::Tamper);
        }
        let directory = directory_under(&self.directory, relative)?;
        let leaf = relative_leaf(relative)?;
        // The partial directory is the anchor. A symlink planted at a parent
        // inside it is not a directory this attempt owns.
        let mut file = open_file_at(&directory, &leaf, OpenMode::CreateNew)?;
        file.write_all(bytes)
            .map_err(|_| RestoreFault::Unreadable)?;
        file.sync_all().map_err(|_| RestoreFault::Unreadable)?;
        Ok(())
    }

    fn finish(mut self: Box<Self>, journal: &str) -> Result<TreeId, RestoreFault> {
        self.write("journal.txt", journal.as_bytes())?;
        sync_tree_at(&self.directory)?;
        let identity = tree_id_of(&self.directory)?;
        rename_exclusive_at(
            &self.parent,
            &self.partial_name,
            &self.parent,
            &self.quarantine_name,
            RestoreFault::QuarantineOccupied,
        )?;
        self.armed = false;
        let published = open_dir_at(&self.parent, &self.quarantine_name)?;
        if tree_id_of(&published)? != identity {
            return Err(RestoreFault::Tamper);
        }
        if sync_dir(&self.parent).is_err() {
            return match remove_opened(&published, &identity) {
                Ok(()) => Err(RestoreFault::Unreadable),
                Err(_) => Err(RestoreFault::Retained),
            };
        }
        Ok(identity)
    }

    fn discard(mut self: Box<Self>) -> Result<(), RestoreFault> {
        self.armed = false;
        remove_opened_tree(&self.directory)
    }
}

impl Drop for PartialTree {
    fn drop(&mut self) {
        if self.armed {
            let _ = remove_opened_tree(&self.directory);
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

/// Remove `path` when it is still `identity`. A different directory stays.
///
/// Children are removed through the open that matched `identity`. A rename
/// of that directory does not make `path` the object that is deleted.
fn remove_same(path: &Path, identity: &TreeId) -> Result<(), RestoreFault> {
    match symlink_metadata_dir(path) {
        Ok(false) => Ok(()),
        Ok(true) => {
            let file = open_directory(path, false).map_err(|_| RestoreFault::Unreadable)?;
            if tree_id_of(&file)? != *identity {
                return Err(RestoreFault::Tamper);
            }
            remove_opened(&file, identity)
        }
        Err(error) => Err(error),
    }
}

/// Delete the directory `file` names. The path is the handle's path, not a
/// later lookup of the caller's path.
fn remove_opened(file: &File, identity: &TreeId) -> Result<(), RestoreFault> {
    if tree_id_of(file)? != *identity {
        return Err(RestoreFault::Tamper);
    }
    let current = path_of_handle(file)?;
    if tree_id(&current)? != *identity {
        return Err(RestoreFault::Tamper);
    }
    fs::remove_dir_all(&current).map_err(|_| RestoreFault::Unreadable)
}

/// Where the open directory is now. A replacement at the caller's path is a
/// different object and is not this path.
fn path_of_handle(file: &File) -> Result<PathBuf, RestoreFault> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::AsRawFd;
        let link = format!("/proc/self/fd/{}", file.as_raw_fd());
        let path = fs::read_link(link).map_err(|_| RestoreFault::Unreadable)?;
        if path.to_string_lossy().ends_with(" (deleted)") {
            return Err(RestoreFault::Unreadable);
        }
        Ok(path)
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::io::AsRawFd;
        let mut buf = vec![0u8; 1024];
        let rc = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, buf.as_mut_ptr()) };
        if rc == -1 {
            return Err(RestoreFault::Unreadable);
        }
        let end = buf.iter().position(|byte| *byte == 0).unwrap_or(buf.len());
        Ok(PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..end])))
    }
    #[cfg(windows)]
    {
        path_of_handle_windows(file)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = file;
        Err(RestoreFault::Unreadable)
    }
}

#[cfg(windows)]
fn path_of_handle_windows(file: &File) -> Result<PathBuf, RestoreFault> {
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetFinalPathNameByHandleW(
            file: *mut std::ffi::c_void,
            buffer: *mut u16,
            size: u32,
            flags: u32,
        ) -> u32;
    }
    let mut buffer = vec![0u16; 1024];
    // The handle is the directory. The call writes its current path.
    let written = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            0,
        )
    };
    if written == 0 || written as usize >= buffer.len() {
        return Err(RestoreFault::Unreadable);
    }
    let text =
        String::from_utf16(&buffer[..written as usize]).map_err(|_| RestoreFault::Unreadable)?;
    Ok(plain_absolute(PathBuf::from(text)))
}

/// Remove the source tree when it is still `identity`. When that tree remains
/// and still names a readiness id, the promotion is retained.
#[cfg(test)]
fn retire_source(path: &Path, identity: &TreeId) -> Result<(), RestoreFault> {
    retire_source_after(path, identity, remove_same)
}

/// Retire the directory `file` still names. A missing original path is not
/// success while this handle names the tree.
fn retire_opened(file: &File, identity: &TreeId) -> Result<(), RestoreFault> {
    match remove_opened(file, identity) {
        Ok(()) => Ok(()),
        Err(RestoreFault::Tamper) => Ok(()),
        Err(_) => {
            if tree_id_of(file).ok().as_ref() != Some(identity) {
                return Ok(());
            }
            let current = path_of_handle(file)?;
            retire_marker(&ready_path(&current))
        }
    }
}

#[cfg(test)]
fn retire_source_after(
    path: &Path,
    identity: &TreeId,
    remove: impl FnOnce(&Path, &TreeId) -> Result<(), RestoreFault>,
) -> Result<(), RestoreFault> {
    match remove(path, identity) {
        Ok(()) => Ok(()),
        Err(RestoreFault::Tamper) => Ok(()),
        Err(_) => {
            if tree_id(path).ok().as_ref() != Some(identity) {
                return Ok(());
            }
            retire_marker(&ready_path(path))
        }
    }
}

fn symlink_metadata_dir(path: &Path) -> Result<bool, RestoreFault> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => Ok(true),
        Ok(_) => Err(RestoreFault::Tamper),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(RestoreFault::Unreadable),
    }
}

/// The directory this attempt created, and the open that names it.
struct SealedDir {
    #[cfg(test)]
    path: PathBuf,
    name: OsString,
    file: File,
    identity: TreeId,
}

#[cfg(test)]
fn sealed_dest(live: &Path) -> Result<SealedDir, RestoreFault> {
    let parent = live
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or(RestoreFault::Paths)?;
    let name = live.file_name().ok_or(RestoreFault::Paths)?.to_os_string();
    for attempt in 0..32 {
        let mut staged = name.clone();
        staged.push(format!(".sealed-{}-{attempt}", std::process::id()));
        let path = parent.join(&staged);
        match mkdir_exclusive(&path) {
            Ok(()) => return adopt_created(&path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(RestoreFault::Unreadable),
        }
    }
    Err(RestoreFault::Unreadable)
}

/// Open the directory `mkdir` just created. A different object at that path
/// is not this attempt, and it is not removed.
#[cfg(test)]
fn adopt_created(path: &Path) -> Result<SealedDir, RestoreFault> {
    let file = open_directory(path, false).map_err(|_| RestoreFault::Unreadable)?;
    let identity = tree_id_of(&file)?;
    if tree_id(path)? != identity || !directory_is_empty(path)? {
        return Err(RestoreFault::Tamper);
    }
    Ok(SealedDir {
        path: path.to_path_buf(),
        name: path.file_name().ok_or(RestoreFault::Paths)?.to_os_string(),
        file,
        identity,
    })
}

/// `created` is still `created_id`, that directory is still at `dest`, and
/// the bytes there are still the seal. A replacement path is not adopted.
#[cfg(test)]
fn copy_still_sealed(
    dest: &Path,
    created: &File,
    created_id: &TreeId,
    loaded: &LoadedTree,
) -> Result<(), RestoreFault> {
    if tree_id_of(created)? != *created_id || tree_id(dest)? != *created_id {
        return Err(RestoreFault::Tamper);
    }
    let journal =
        fs::read_to_string(dest.join("journal.txt")).map_err(|_| RestoreFault::Unreadable)?;
    if journal != loaded.journal || content_token(dest)? != loaded.content {
        return Err(RestoreFault::Tamper);
    }
    Ok(())
}

#[cfg(test)]
fn directory_is_empty(path: &Path) -> Result<bool, RestoreFault> {
    let mut entries = fs::read_dir(path).map_err(|_| RestoreFault::Unreadable)?;
    match entries.next() {
        None => Ok(true),
        Some(Ok(_)) => Ok(false),
        Some(Err(_)) => Err(RestoreFault::Unreadable),
    }
}

/// Write the readiness marker, rename it into place, then sync `root`.
///
/// A failed directory sync clears the marker. A partial file is not readiness.
#[cfg(test)]
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

#[cfg(test)]
fn retire_both(temp: &Path, path: &Path, cleared: RestoreFault) -> Result<(), RestoreFault> {
    let temp_gone = retire_marker(temp);
    let path_gone = retire_marker(path);
    if temp_gone.is_err() || path_gone.is_err() {
        return Err(RestoreFault::Retained);
    }
    Err(cleared)
}

#[cfg(test)]
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

/// Every file except the journal and the readiness marker. Promotion refuses
/// when this changes after the tree was read.
#[cfg(test)]
fn content_token(root: &Path) -> Result<Fingerprint, RestoreFault> {
    let mut lines = Vec::new();
    collect_content(root, root, &mut lines)?;
    lines.sort();
    Ok(Fingerprint::of(lines.join("\n").as_bytes()))
}

#[cfg(test)]
fn collect_content(
    root: &Path,
    directory: &Path,
    lines: &mut Vec<String>,
) -> Result<(), RestoreFault> {
    for entry in fs::read_dir(directory).map_err(|_| RestoreFault::Unreadable)? {
        let path = entry.map_err(|_| RestoreFault::Unreadable)?.path();
        let meta = fs::symlink_metadata(&path).map_err(|_| RestoreFault::Unreadable)?;
        if meta.file_type().is_symlink() {
            return Err(RestoreFault::Tamper);
        }
        if meta.is_dir() {
            collect_content(root, &path, lines)?;
            continue;
        }
        if !meta.is_file() {
            return Err(RestoreFault::Tamper);
        }
        let relative = relative_to(root, &path)?;
        if relative == "journal.txt" || relative == "accept-ready" {
            continue;
        }
        lines.push(format!("{} {}", relative, hash_file(&path)?.hex()));
    }
    Ok(())
}

#[cfg(test)]
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

static PARTIAL_GATE: Mutex<()> = Mutex::new(());
static ATTEMPT: AtomicU64 = AtomicU64::new(1);

/// Create a partial directory this attempt owns, under `parent`. An existing
/// name is left alone.
fn claim_partial_locked(
    parent: &File,
    quarantine_name: &OsStr,
    live: &Path,
    quarantine_probe: VolumeProbe,
    live_probe: VolumeProbe,
) -> Result<(File, OsString), RestoreFault> {
    for _ in 0..32 {
        let mut partial_name = quarantine_name.to_os_string();
        partial_name.push(format!(
            ".partial-{}-{}",
            std::process::id(),
            ATTEMPT.fetch_add(1, Ordering::Relaxed)
        ));
        let partial_path = path_of_handle(parent)?.join(&partial_name);
        if paths_conflict_on(&partial_path, live, quarantine_probe, live_probe).is_err() {
            continue;
        }
        match mkdir_exclusive_at(parent, &partial_name) {
            Ok(()) => {
                let directory = open_dir_at(parent, &partial_name)?;
                return Ok((directory, partial_name));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(RestoreFault::Unreadable),
        }
    }
    Err(RestoreFault::Unreadable)
}

fn tree_id(path: &Path) -> Result<TreeId, RestoreFault> {
    #[cfg(windows)]
    refuse_reparse(path)?;
    let file = open_directory(path, false).map_err(|_| RestoreFault::Unreadable)?;
    tree_id_of(&file)
}

/// Open `path` as a directory.
///
/// `follow` is the Windows reparse choice. Unix `open` already follows, and a
/// directory open needs no extra flag. Windows `CreateFileW` refuses a
/// directory unless `FILE_FLAG_BACKUP_SEMANTICS` is set. `follow` false also
/// sets `FILE_FLAG_OPEN_REPARSE_POINT`, so the handle is the link itself.
fn open_directory(path: &Path, follow: bool) -> std::io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        let mut flags = FILE_FLAG_BACKUP_SEMANTICS;
        if !follow {
            flags |= FILE_FLAG_OPEN_REPARSE_POINT;
        }
        options.custom_flags(flags);
    }
    #[cfg(not(windows))]
    {
        let _ = follow;
    }
    options.open(path)
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Missing,
    File,
    Directory,
    Symlink,
    Other,
}

fn parent_of_root(root: &AnchoredRoot) -> Result<(File, OsString), RestoreFault> {
    if root.tail.is_empty() {
        return Err(RestoreFault::Paths);
    }
    let mut directory = duplicate(&root.ancestor)?;
    let last = root.tail.len() - 1;
    for name in &root.tail[..last] {
        directory = open_dir_at(&directory, name)?;
    }
    Ok((directory, root.tail[last].clone()))
}

fn open_root_dir(root: &AnchoredRoot) -> Result<File, RestoreFault> {
    let (parent, name) = parent_of_root(root)?;
    open_dir_at(&parent, &name)
}

fn relative_leaf(relative: &str) -> Result<OsString, RestoreFault> {
    if !relative_path_is_safe(relative) {
        return Err(RestoreFault::Tamper);
    }
    Ok(Path::new(relative)
        .file_name()
        .ok_or(RestoreFault::Tamper)?
        .to_os_string())
}

/// Parents of `relative`, created when they are missing. The leaf is not opened.
fn directory_under(root: &File, relative: &str) -> Result<File, RestoreFault> {
    descend(root, relative, true)
}

fn directory_for(root: &AnchoredRoot, relative: &str) -> Result<File, RestoreFault> {
    descend(&open_root_dir(root)?, relative, false)
}

fn descend(root: &File, relative: &str, create: bool) -> Result<File, RestoreFault> {
    if !relative_path_is_safe(relative) {
        return Err(RestoreFault::Tamper);
    }
    let mut directory = duplicate(root)?;
    let path = Path::new(relative);
    if let Some(parent) = path.parent() {
        for component in parent.components() {
            let std::path::Component::Normal(name) = component else {
                return Err(RestoreFault::Tamper);
            };
            directory = if create {
                ensure_dir_at(&directory, name)?
            } else {
                open_dir_at(&directory, name)?
            };
        }
    }
    Ok(directory)
}

fn ensure_dir_at(parent: &File, name: &OsStr) -> Result<File, RestoreFault> {
    match kind_at(parent, name)? {
        EntryKind::Missing => {
            mkdir_exclusive_at(parent, name).map_err(|_| RestoreFault::Unreadable)?;
            open_dir_at(parent, name)
        }
        EntryKind::Directory => open_dir_at(parent, name),
        EntryKind::File | EntryKind::Symlink | EntryKind::Other => Err(RestoreFault::Unreadable),
    }
}

fn read_file_at(directory: &File, name: &OsStr) -> Result<String, RestoreFault> {
    let bytes = read_bytes_at(directory, name)?;
    String::from_utf8(bytes).map_err(|_| RestoreFault::Unreadable)
}

fn read_bytes_at(directory: &File, name: &OsStr) -> Result<Vec<u8>, RestoreFault> {
    let mut file = open_file_at(directory, name, OpenMode::Read)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| RestoreFault::Unreadable)?;
    Ok(bytes)
}

fn hash_at(directory: &File, name: &OsStr) -> Result<Fingerprint, RestoreFault> {
    let mut file = open_file_at(directory, name, OpenMode::Read)?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| RestoreFault::Unreadable)?;
        if count == 0 {
            break;
        }
        sha2::Digest::update(&mut hasher, &buffer[..count]);
    }
    Ok(Fingerprint::from_digest(hasher.finalize().into()))
}

fn child_relative(prefix: &str, name: &OsStr) -> Result<String, RestoreFault> {
    let name = name.to_str().ok_or(RestoreFault::Tamper)?;
    let relative = if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}/{name}")
    };
    if !relative_path_is_safe(&relative) {
        return Err(RestoreFault::Tamper);
    }
    Ok(relative)
}

fn walk_at(
    directory: &File,
    prefix: &str,
    files: &mut Vec<SealedFile>,
    extras: &mut Vec<String>,
    helpers: &mut Vec<String>,
) -> Result<(), RestoreFault> {
    for name in list_names(directory)? {
        let relative = child_relative(prefix, &name)?;
        match kind_at(directory, &name)? {
            EntryKind::Directory => {
                let child = open_dir_at(directory, &name)?;
                walk_at(&child, &relative, files, extras, helpers)?;
            }
            EntryKind::File => {
                if relative == "journal.txt" || relative == "accept-ready" {
                    continue;
                }
                if session_sidecar(&relative).is_some() {
                    helpers.push(relative);
                    continue;
                }
                let fingerprint = hash_at(directory, &name)?;
                if relative.starts_with("sessions/") {
                    extras.push(relative);
                } else {
                    files.push(SealedFile {
                        path: relative,
                        fingerprint,
                    });
                }
            }
            EntryKind::Symlink | EntryKind::Other => return Err(RestoreFault::Tamper),
            EntryKind::Missing => return Err(RestoreFault::Unreadable),
        }
    }
    Ok(())
}

fn content_of(directory: &File) -> Result<Fingerprint, RestoreFault> {
    let mut lines = Vec::new();
    collect_at(directory, "", &mut lines)?;
    lines.sort();
    Ok(Fingerprint::of(lines.join("\n").as_bytes()))
}

fn collect_at(directory: &File, prefix: &str, lines: &mut Vec<String>) -> Result<(), RestoreFault> {
    for name in list_names(directory)? {
        let relative = child_relative(prefix, &name)?;
        match kind_at(directory, &name)? {
            EntryKind::Directory => {
                let child = open_dir_at(directory, &name)?;
                collect_at(&child, &relative, lines)?;
            }
            EntryKind::File => {
                if relative == "journal.txt" || relative == "accept-ready" {
                    continue;
                }
                lines.push(format!("{relative} {}", hash_at(directory, &name)?.hex()));
            }
            EntryKind::Symlink | EntryKind::Other => return Err(RestoreFault::Tamper),
            EntryKind::Missing => return Err(RestoreFault::Unreadable),
        }
    }
    Ok(())
}

fn copy_at(
    source: &File,
    dest: &File,
    held: &mut Vec<Unpublished>,
) -> Result<Fingerprint, RestoreFault> {
    let mut lines = Vec::new();
    copy_entries(source, dest, "", &mut lines, held)?;
    lines.sort();
    Ok(Fingerprint::of(lines.join("\n").as_bytes()))
}

fn copy_entries(
    source: &File,
    dest: &File,
    prefix: &str,
    lines: &mut Vec<String>,
    held: &mut Vec<Unpublished>,
) -> Result<(), RestoreFault> {
    for name in list_names(source)? {
        let relative = child_relative(prefix, &name)?;
        match kind_at(source, &name)? {
            EntryKind::Directory => {
                let child = open_dir_at(source, &name)?;
                let created = ensure_dir_at(dest, &name)?;
                copy_entries(&child, &created, &relative, lines, held)?;
            }
            EntryKind::File => {
                if relative == "journal.txt" || relative == "accept-ready" {
                    continue;
                }
                let fingerprint = copy_bytes_at(source, dest, &name, &relative, held)?;
                lines.push(format!("{relative} {}", fingerprint.hex()));
            }
            EntryKind::Symlink | EntryKind::Other => return Err(RestoreFault::Tamper),
            EntryKind::Missing => return Err(RestoreFault::Unreadable),
        }
    }
    Ok(())
}

fn copy_bytes_at(
    source: &File,
    dest: &File,
    name: &OsStr,
    relative: &str,
    held: &mut Vec<Unpublished>,
) -> Result<Fingerprint, RestoreFault> {
    let mut input = open_file_at(source, name, OpenMode::Read)?;
    let mut output = create_unpublished(dest, name, relative, false, RestoreFault::Tamper)?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let count = input
            .read(&mut buffer)
            .map_err(|_| RestoreFault::Unreadable)?;
        if count == 0 {
            break;
        }
        sha2::Digest::update(&mut hasher, &buffer[..count]);
        output.store(&buffer[..count])?;
    }
    output.sync_file()?;
    let fingerprint = Fingerprint::from_digest(hasher.finalize().into());
    output.fingerprint = fingerprint;
    held.push(output);
    Ok(fingerprint)
}

/// A file this attempt wrote and still holds.
///
/// On Linux and macOS it has no directory entry until [`Unpublished::publish_in`].
/// On Windows it is `name` in `parent`, and the handle does not share write
/// access, so another open cannot change the bytes.
struct Unpublished {
    parent: File,
    name: OsString,
    relative: String,
    file: Option<File>,
    fingerprint: Fingerprint,
    /// Windows created `name`. Unix did not.
    named: bool,
    /// The name is the one publication keeps. Drop must not remove it.
    published: bool,
}

impl Unpublished {
    fn file_mut(&mut self) -> Result<&mut File, RestoreFault> {
        self.file.as_mut().ok_or(RestoreFault::Unreadable)
    }

    fn store(&mut self, bytes: &[u8]) -> Result<(), RestoreFault> {
        self.file_mut()?
            .write_all(bytes)
            .map_err(|_| RestoreFault::Unreadable)
    }

    fn sync_file(&mut self) -> Result<(), RestoreFault> {
        self.file_mut()?
            .sync_all()
            .map_err(|_| RestoreFault::Unreadable)
    }

    fn rehash(&mut self) -> Result<Fingerprint, RestoreFault> {
        let file = self.file_mut()?;
        file.seek(SeekFrom::Start(0))
            .map_err(|_| RestoreFault::Unreadable)?;
        let mut hasher = sha2::Sha256::new();
        let mut buffer = [0u8; 8192];
        loop {
            let count = file
                .read(&mut buffer)
                .map_err(|_| RestoreFault::Unreadable)?;
            if count == 0 {
                break;
            }
            sha2::Digest::update(&mut hasher, &buffer[..count]);
        }
        Ok(Fingerprint::from_digest(hasher.finalize().into()))
    }

    fn read_back(&mut self) -> Result<Vec<u8>, RestoreFault> {
        let file = self.file_mut()?;
        file.seek(SeekFrom::Start(0))
            .map_err(|_| RestoreFault::Unreadable)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|_| RestoreFault::Unreadable)?;
        Ok(bytes)
    }

    /// Path SQLite can open while this handle denies other writers.
    #[cfg(not(unix))]
    fn bytes_path(&self) -> Result<PathBuf, RestoreFault> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::io::AsRawFd;
            let file = self.file.as_ref().ok_or(RestoreFault::Unreadable)?;
            Ok(PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd())))
        }
        #[cfg(all(unix, not(target_os = "linux")))]
        {
            use std::os::unix::io::AsRawFd;
            let file = self.file.as_ref().ok_or(RestoreFault::Unreadable)?;
            Ok(PathBuf::from(format!("/dev/fd/{}", file.as_raw_fd())))
        }
        #[cfg(windows)]
        {
            Ok(path_of_handle(&self.parent)?.join(&self.name))
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = self;
            Err(RestoreFault::Unreadable)
        }
    }

    /// Give `name` to this inode. On Windows the name already exists.
    fn publish_in(&mut self, occupied: RestoreFault) -> Result<(), RestoreFault> {
        #[cfg(unix)]
        {
            let file = self.file.as_ref().ok_or(RestoreFault::Unreadable)?;
            link_unpublished(file, &self.parent, &self.name, occupied)?;
        }
        #[cfg(windows)]
        {
            let _ = occupied;
        }
        self.published = true;
        Ok(())
    }
}

impl Drop for Unpublished {
    fn drop(&mut self) {
        // Close first. A Windows handle that does not share delete cannot be
        // removed while it is still open.
        self.file.take();
        if self.published || !self.named {
            return;
        }
        let _ = unlink_at(&self.parent, &self.name);
    }
}

fn unpublished_matches(
    output: &mut Unpublished,
    conversation_id: &str,
    digest: &Fingerprint,
) -> Result<bool, RestoreFault> {
    #[cfg(unix)]
    {
        let matched = match inspect_open_file(output.file_mut()?, conversation_id) {
            Ok(stream) if Fingerprint::from_digest(stream.digest) == *digest => true,
            Ok(_) | Err(_) => false,
        };
        Ok(matched)
    }
    #[cfg(not(unix))]
    {
        let staged = output.bytes_path()?;
        let matched = match inspect_conversation(&staged, conversation_id) {
            Ok(stream) if Fingerprint::from_digest(stream.digest) == *digest => true,
            Ok(_) | Err(_) => false,
        };
        Ok(matched)
    }
}

fn create_unpublished(
    parent: &File,
    name: &OsStr,
    relative: &str,
    private: bool,
    occupied: RestoreFault,
) -> Result<Unpublished, RestoreFault> {
    let parent = duplicate(parent)?;
    #[cfg(not(windows))]
    {
        let _ = (private, occupied);
    }
    #[cfg(target_os = "linux")]
    let file = {
        use std::os::unix::io::{AsRawFd, FromRawFd};
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                c".".as_ptr(),
                libc::O_TMPFILE | libc::O_RDWR | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(RestoreFault::Unreadable);
        }
        unsafe { File::from_raw_fd(fd) }
    };
    #[cfg(all(unix, not(target_os = "linux")))]
    let file = {
        let mut temp_name = OsString::from(".held-");
        temp_name.push(std::process::id().to_string());
        temp_name.push("-");
        temp_name.push(ATTEMPT.fetch_add(1, Ordering::Relaxed).to_string());
        let created = open_file_at(&parent, &temp_name, OpenMode::CreateNew)?;
        unlink_at(&parent, &temp_name)?;
        created
    };
    #[cfg(windows)]
    let file = {
        let path = path_of_handle(&parent)?.join(name);
        let opened = if private {
            create_no_writers(&path)
        } else {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_SHARE_READ: u32 = 0x0000_0001;
            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .share_mode(FILE_SHARE_READ)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&path)
        };
        match opened {
            Ok(file) => {
                let _ = windows_file_id(&file)?;
                file
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(occupied);
            }
            Err(_) => return Err(RestoreFault::Unreadable),
        }
    };
    #[cfg(not(any(unix, windows)))]
    let file = {
        let _ = (parent, name);
        return Err(RestoreFault::Unreadable);
    };
    Ok(Unpublished {
        parent,
        name: name.to_os_string(),
        relative: relative.to_owned(),
        file: Some(file),
        fingerprint: Fingerprint::of(b""),
        named: cfg!(windows),
        published: false,
    })
}

#[cfg(target_os = "linux")]
fn link_unpublished(
    file: &File,
    parent: &File,
    name: &OsStr,
    occupied: RestoreFault,
) -> Result<(), RestoreFault> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::io::AsRawFd;
    let proc = format!("/proc/self/fd/{}", file.as_raw_fd());
    let source = std::ffi::CString::new(proc).map_err(|_| RestoreFault::Unreadable)?;
    let target = std::ffi::CString::new(name.as_bytes()).map_err(|_| RestoreFault::Unreadable)?;
    let linked = unsafe {
        libc::linkat(
            libc::AT_FDCWD,
            source.as_ptr(),
            parent.as_raw_fd(),
            target.as_ptr(),
            libc::AT_SYMLINK_FOLLOW,
        )
    };
    if linked != 0 {
        let error = std::io::Error::last_os_error();
        return if error.kind() == std::io::ErrorKind::AlreadyExists {
            Err(occupied)
        } else {
            Err(RestoreFault::Unreadable)
        };
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "linux")))]
fn link_unpublished(
    file: &File,
    parent: &File,
    name: &OsStr,
    occupied: RestoreFault,
) -> Result<(), RestoreFault> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::io::AsRawFd;
    let target = std::ffi::CString::new(name.as_bytes()).map_err(|_| RestoreFault::Unreadable)?;
    let cloned =
        unsafe { libc::fclonefileat(file.as_raw_fd(), parent.as_raw_fd(), target.as_ptr(), 0) };
    if cloned != 0 {
        let error = std::io::Error::last_os_error();
        return if error.kind() == std::io::ErrorKind::AlreadyExists {
            Err(occupied)
        } else {
            Err(RestoreFault::Unreadable)
        };
    }
    Ok(())
}

fn publish_held(held: &mut [Unpublished], journal: &mut Unpublished) -> Result<(), RestoreFault> {
    for file in held.iter_mut() {
        file.publish_in(RestoreFault::Tamper)?;
    }
    journal.publish_in(RestoreFault::Tamper)
}

fn held_still_sealed(
    directory: &File,
    identity: &TreeId,
    loaded: &LoadedTree,
    held: &mut [Unpublished],
    journal: &mut Unpublished,
) -> Result<(), RestoreFault> {
    if tree_id_of(directory)? != *identity {
        return Err(RestoreFault::Tamper);
    }
    if journal.read_back()? != loaded.journal.as_bytes() {
        return Err(RestoreFault::Tamper);
    }
    let mut lines = Vec::new();
    for file in held.iter_mut() {
        let fingerprint = file.rehash()?;
        if fingerprint != file.fingerprint {
            return Err(RestoreFault::Tamper);
        }
        lines.push(format!("{} {}", file.relative, fingerprint.hex()));
    }
    lines.sort();
    if Fingerprint::of(lines.join("\n").as_bytes()) != loaded.content {
        return Err(RestoreFault::Tamper);
    }
    // Unix files are not named yet. A named file is one this attempt did not write.
    #[cfg(unix)]
    if contains_file(directory)? {
        return Err(RestoreFault::Tamper);
    }
    // Windows names the files while the handles deny writers. An extra name is
    // not the seal. The check does not reopen the files.
    #[cfg(windows)]
    {
        let mut expected: Vec<String> = held.iter().map(|file| file.relative.clone()).collect();
        expected.push(String::from("journal.txt"));
        if unexpected_file(directory, "", &expected)? {
            return Err(RestoreFault::Tamper);
        }
    }
    Ok(())
}

#[cfg(windows)]
fn unexpected_file(
    directory: &File,
    prefix: &str,
    expected: &[String],
) -> Result<bool, RestoreFault> {
    for name in list_names(directory)? {
        let relative = child_relative(prefix, &name)?;
        match kind_at(directory, &name)? {
            EntryKind::Directory => {
                if unexpected_file(&open_dir_at(directory, &name)?, &relative, expected)? {
                    return Ok(true);
                }
            }
            EntryKind::File => {
                if !expected.iter().any(|item| item == &relative) {
                    return Ok(true);
                }
            }
            EntryKind::Symlink | EntryKind::Other => return Ok(true),
            EntryKind::Missing => return Err(RestoreFault::Unreadable),
        }
    }
    Ok(false)
}

#[cfg(unix)]
fn contains_file(directory: &File) -> Result<bool, RestoreFault> {
    for name in list_names(directory)? {
        match kind_at(directory, &name)? {
            EntryKind::File | EntryKind::Symlink | EntryKind::Other => return Ok(true),
            EntryKind::Directory => {
                if contains_file(&open_dir_at(directory, &name)?)? {
                    return Ok(true);
                }
            }
            EntryKind::Missing => return Err(RestoreFault::Unreadable),
        }
    }
    Ok(false)
}

fn sync_tree_at(directory: &File) -> Result<(), RestoreFault> {
    for name in list_names(directory)? {
        if kind_at(directory, &name)? == EntryKind::Directory {
            let child = open_dir_at(directory, &name)?;
            sync_tree_at(&child)?;
        }
    }
    sync_dir(directory)
}

/// Directory durability. Windows has no directory fsync; a read-only handle
/// fails `FlushFileBuffers`, so this is the same no-op as `sync_directory`.
fn sync_dir(directory: &File) -> Result<(), RestoreFault> {
    #[cfg(windows)]
    {
        let _ = directory;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        directory.sync_all().map_err(|_| RestoreFault::Unreadable)
    }
}

fn sealed_beside(parent: &File, live_name: &OsStr) -> Result<SealedDir, RestoreFault> {
    for attempt in 0..32 {
        let mut name = live_name.to_os_string();
        name.push(format!(".sealed-{}-{attempt}", std::process::id()));
        match mkdir_exclusive_at(parent, &name) {
            Ok(()) => {
                let file = open_dir_at(parent, &name)?;
                let identity = tree_id_of(&file)?;
                if !list_names(&file)?.is_empty() {
                    return Err(RestoreFault::Tamper);
                }
                #[cfg(test)]
                let path = path_of_handle(&file)?;
                return Ok(SealedDir {
                    #[cfg(test)]
                    path,
                    name,
                    file,
                    identity,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(RestoreFault::Unreadable),
        }
    }
    Err(RestoreFault::Unreadable)
}

fn remove_opened_tree(file: &File) -> Result<(), RestoreFault> {
    let current = path_of_handle(file)?;
    match fs::remove_dir_all(&current) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) if !current.exists() => Ok(()),
        Err(_) => Err(RestoreFault::Retained),
    }
}

fn publish_marker_at(directory: &File, body: &[u8]) -> Result<(), RestoreFault> {
    let partial = OsStr::new("accept-ready.partial");
    let marker = OsStr::new("accept-ready");
    if name_exists_at(directory, partial)? {
        unlink_at(directory, partial)?;
    }
    if name_exists_at(directory, marker)? {
        unlink_at(directory, marker)?;
    }
    let mut file = open_file_at(directory, partial, OpenMode::CreateNew)?;
    let synced = file.write_all(body).and_then(|_| file.sync_all());
    drop(file);
    if synced.is_err() {
        let _ = unlink_at(directory, partial);
        return Err(RestoreFault::Unreadable);
    }
    if rename_exclusive_at(
        directory,
        partial,
        directory,
        marker,
        RestoreFault::Unreadable,
    )
    .is_err()
    {
        let _ = unlink_at(directory, partial);
        let _ = unlink_at(directory, marker);
        return Err(RestoreFault::Unreadable);
    }
    if sync_dir(directory).is_err() {
        return match unlink_at(directory, marker) {
            Ok(()) => Err(RestoreFault::Unreadable),
            Err(_) => Err(RestoreFault::Retained),
        };
    }
    Ok(())
}

fn read_marker_bytes(bytes: &[u8]) -> Result<Option<String>, RestoreFault> {
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

fn probe_of(file: &File) -> Result<VolumeProbe, RestoreFault> {
    Ok(VolumeProbe {
        device: device_of_file(file)?,
        case: case_of_file(file)?,
    })
}

fn device_of_file(file: &File) -> Result<u64, RestoreFault> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(file.metadata().map_err(|_| RestoreFault::Unreadable)?.dev())
    }
    #[cfg(windows)]
    {
        Ok(windows_file_id(file)?.0)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Ok(0)
    }
}

fn duplicate(file: &File) -> Result<File, RestoreFault> {
    #[cfg(unix)]
    {
        use std::os::unix::io::{AsRawFd, FromRawFd};
        let fd = unsafe { libc::dup(file.as_raw_fd()) };
        if fd < 0 {
            return Err(RestoreFault::Unreadable);
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::{AsRawHandle, FromRawHandle};
        #[link(name = "kernel32")]
        extern "system" {
            fn DuplicateHandle(
                source_process: *mut std::ffi::c_void,
                source: *mut std::ffi::c_void,
                target_process: *mut std::ffi::c_void,
                target: *mut *mut std::ffi::c_void,
                access: u32,
                inherit: i32,
                options: u32,
            ) -> i32;
            fn GetCurrentProcess() -> *mut std::ffi::c_void;
        }
        const DUPLICATE_SAME_ACCESS: u32 = 0x0000_0002;
        let mut target = std::ptr::null_mut();
        let copied = unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                file.as_raw_handle(),
                GetCurrentProcess(),
                &mut target,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        if copied == 0 {
            return Err(RestoreFault::Unreadable);
        }
        Ok(unsafe { File::from_raw_handle(target) })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Err(RestoreFault::Unreadable)
    }
}

fn open_ancestor(path: &Path) -> Result<File, RestoreFault> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(path)
            .map_err(|_| RestoreFault::Unreadable)?;
        let meta = file.metadata().map_err(|_| RestoreFault::Unreadable)?;
        if !meta.is_dir() {
            return Err(RestoreFault::Unreadable);
        }
        Ok(file)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        // Follow the one legitimate directory, then hold it without delete
        // share so it cannot be renamed for the rest of this anchor.
        let file = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
            .map_err(|_| RestoreFault::Unreadable)?;
        let _ = windows_file_id(&file)?;
        Ok(file)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(RestoreFault::Unreadable)
    }
}

fn open_dir_at(parent: &File, name: &OsStr) -> Result<File, RestoreFault> {
    #[cfg(unix)]
    {
        let file = open_at(
            parent,
            name,
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC,
            0,
        )?;
        Ok(file)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        let path = path_of_handle(parent)?.join(name);
        let file = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|_| RestoreFault::Unreadable)?;
        let _ = windows_file_id(&file)?;
        Ok(file)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (parent, name);
        Err(RestoreFault::Unreadable)
    }
}

fn open_file_at(parent: &File, name: &OsStr, mode: OpenMode) -> Result<File, RestoreFault> {
    #[cfg(unix)]
    {
        let mut flags = libc::O_CLOEXEC | libc::O_NOFOLLOW;
        flags |= match mode {
            OpenMode::Read => libc::O_RDONLY,
            OpenMode::ReadNonblocking => libc::O_RDONLY | libc::O_NONBLOCK,
            OpenMode::ReadWrite => libc::O_RDWR,
            OpenMode::OpenOrCreate => libc::O_RDWR | libc::O_CREAT,
            OpenMode::CreateNew => libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
        };
        let file = open_at(parent, name, flags, 0o600)?;
        let meta = file.metadata().map_err(|_| RestoreFault::Unreadable)?;
        if !meta.is_file() {
            return Err(RestoreFault::Unreadable);
        }
        Ok(file)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        let path = path_of_handle(parent)?.join(name);
        let mut options = fs::OpenOptions::new();
        options
            .read(true)
            .write(!matches!(mode, OpenMode::Read | OpenMode::ReadNonblocking))
            .create(matches!(mode, OpenMode::OpenOrCreate))
            .create_new(matches!(mode, OpenMode::CreateNew))
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        let file = options.open(path).map_err(|_| RestoreFault::Unreadable)?;
        let _ = windows_file_id(&file)?;
        let meta = file.metadata().map_err(|_| RestoreFault::Unreadable)?;
        if !meta.is_file() {
            return Err(RestoreFault::Tamper);
        }
        Ok(file)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (parent, name, mode);
        Err(RestoreFault::Unreadable)
    }
}

#[cfg(unix)]
fn open_at(parent: &File, name: &OsStr, flags: i32, mode: u32) -> Result<File, RestoreFault> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::io::{AsRawFd, FromRawFd};
    let c_name = std::ffi::CString::new(name.as_bytes()).map_err(|_| RestoreFault::Unreadable)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            c_name.as_ptr(),
            flags,
            mode as libc::c_uint,
        )
    };
    if fd < 0 {
        return Err(RestoreFault::Unreadable);
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn mkdir_exclusive_at(parent: &File, name: &OsStr) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::io::AsRawFd;
        let c_name = std::ffi::CString::new(name.as_bytes())
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "directory name"))?;
        if unsafe { libc::mkdirat(parent.as_raw_fd(), c_name.as_ptr(), 0o700) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        let path = path_of_handle(parent)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "directory handle"))?;
        create_directory_exclusive(&path.join(name))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (parent, name);
        Err(std::io::Error::other("unsupported"))
    }
}

fn name_exists_at(parent: &File, name: &OsStr) -> Result<bool, RestoreFault> {
    Ok(kind_at(parent, name)? != EntryKind::Missing)
}

fn kind_at(parent: &File, name: &OsStr) -> Result<EntryKind, RestoreFault> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::io::AsRawFd;
        let c_name =
            std::ffi::CString::new(name.as_bytes()).map_err(|_| RestoreFault::Unreadable)?;
        let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
        let read = unsafe {
            libc::fstatat(
                parent.as_raw_fd(),
                c_name.as_ptr(),
                &mut stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if read != 0 {
            let error = std::io::Error::last_os_error();
            return if error.kind() == std::io::ErrorKind::NotFound {
                Ok(EntryKind::Missing)
            } else {
                Err(RestoreFault::Unreadable)
            };
        }
        let mode = stat.st_mode & libc::S_IFMT;
        if mode == libc::S_IFLNK {
            Ok(EntryKind::Symlink)
        } else if mode == libc::S_IFDIR {
            Ok(EntryKind::Directory)
        } else if mode == libc::S_IFREG {
            Ok(EntryKind::File)
        } else {
            Ok(EntryKind::Other)
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        let path = path_of_handle(parent)?.join(name);
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 => {
                Ok(EntryKind::Symlink)
            }
            Ok(meta) if meta.is_dir() => Ok(EntryKind::Directory),
            Ok(meta) if meta.is_file() => Ok(EntryKind::File),
            Ok(_) => Ok(EntryKind::Other),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(EntryKind::Missing),
            Err(_) => Err(RestoreFault::Unreadable),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (parent, name);
        Err(RestoreFault::Unreadable)
    }
}

fn list_names(directory: &File) -> Result<Vec<OsString>, RestoreFault> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::io::AsRawFd;
        // A new file description. `dup` shares the directory offset, so a
        // second listing of the same handle would see an empty directory.
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                c".".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(RestoreFault::Unreadable);
        }
        let dir = unsafe { libc::fdopendir(fd) };
        if dir.is_null() {
            unsafe { libc::close(fd) };
            return Err(RestoreFault::Unreadable);
        }
        let mut names = Vec::new();
        loop {
            let entry = unsafe { libc::readdir(dir) };
            if entry.is_null() {
                break;
            }
            let bytes = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            names.push(OsStr::from_bytes(bytes).to_os_string());
        }
        unsafe { libc::closedir(dir) };
        Ok(names)
    }
    #[cfg(windows)]
    {
        let path = path_of_handle(directory)?;
        let mut names = Vec::new();
        for entry in fs::read_dir(path).map_err(|_| RestoreFault::Unreadable)? {
            let entry = entry.map_err(|_| RestoreFault::Unreadable)?;
            names.push(entry.file_name());
        }
        Ok(names)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = directory;
        Err(RestoreFault::Unreadable)
    }
}

fn unlink_at(parent: &File, name: &OsStr) -> Result<(), RestoreFault> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::io::AsRawFd;
        let c_name =
            std::ffi::CString::new(name.as_bytes()).map_err(|_| RestoreFault::Unreadable)?;
        if unsafe { libc::unlinkat(parent.as_raw_fd(), c_name.as_ptr(), 0) } != 0 {
            let error = std::io::Error::last_os_error();
            return if error.kind() == std::io::ErrorKind::NotFound {
                Ok(())
            } else {
                Err(RestoreFault::Unreadable)
            };
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        let path = path_of_handle(parent)?.join(name);
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(RestoreFault::Unreadable),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (parent, name);
        Err(RestoreFault::Unreadable)
    }
}

fn rename_exclusive_at(
    from_dir: &File,
    from: &OsStr,
    to_dir: &File,
    to: &OsStr,
    occupied: RestoreFault,
) -> Result<(), RestoreFault> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::io::AsRawFd;
        let from_name =
            std::ffi::CString::new(from.as_bytes()).map_err(|_| RestoreFault::Unreadable)?;
        let to_name =
            std::ffi::CString::new(to.as_bytes()).map_err(|_| RestoreFault::Unreadable)?;
        #[cfg(target_vendor = "apple")]
        let renamed = unsafe {
            libc::renameatx_np(
                from_dir.as_raw_fd(),
                from_name.as_ptr(),
                to_dir.as_raw_fd(),
                to_name.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        #[cfg(target_os = "linux")]
        let renamed = unsafe {
            libc::renameat2(
                from_dir.as_raw_fd(),
                from_name.as_ptr(),
                to_dir.as_raw_fd(),
                to_name.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
        let renamed = {
            let _ = (from_dir, to_dir, from_name, to_name);
            -1
        };
        if renamed != 0 {
            let error = std::io::Error::last_os_error();
            return if error.kind() == std::io::ErrorKind::AlreadyExists {
                Err(occupied)
            } else {
                Err(RestoreFault::Rename)
            };
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        let from_path = path_of_handle(from_dir)?.join(from);
        let to_path = path_of_handle(to_dir)?.join(to);
        rename_new(&from_path, &to_path, occupied)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (from_dir, from, to_dir, to, occupied);
        Err(RestoreFault::Rename)
    }
}

fn case_of_file(file: &File) -> Result<VolumeCase, RestoreFault> {
    #[cfg(target_os = "linux")]
    {
        linux_case_file(file)
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::io::AsRawFd;
        let sensitive = unsafe { libc::fpathconf(file.as_raw_fd(), libc::_PC_CASE_SENSITIVE) };
        if sensitive == 1 {
            Ok(VolumeCase::Bytes)
        } else {
            Ok(VolumeCase::UnicodeDecomposed)
        }
    }
    #[cfg(windows)]
    {
        let _ = file;
        Ok(VolumeCase::Unicode)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = file;
        Ok(VolumeCase::Bytes)
    }
}

/// Decode a `/proc/self/mountinfo` field. Octal escapes are three digits.
#[cfg(any(test, target_os = "linux"))]
fn decode_mount_field(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 3 < bytes.len() {
            let oct = &bytes[index + 1..index + 4];
            if oct.iter().all(|byte| (b'0'..=b'7').contains(byte)) {
                let value = (oct[0] - b'0') * 64 + (oct[1] - b'0') * 8 + (oct[2] - b'0');
                out.push(value);
                index += 4;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Filesystem type of `target` in a mountinfo table. The longest decoded
/// mount point wins.
#[cfg(any(test, target_os = "linux"))]
fn fstype_at_mount(text: &str, target: &str) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for line in text.lines() {
        let Some((pre, post)) = line.split_once(" - ") else {
            continue;
        };
        let Some(raw) = pre.split_whitespace().nth(4) else {
            continue;
        };
        let mount = decode_mount_field(raw);
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
        let file = open_directory(dir, true).map_err(|_| RestoreFault::Unreadable)?;
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
/// Linux calls this from `statfs`. Other hosts keep it for the same table.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
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
    let file = File::open(dir).map_err(|_| RestoreFault::Unreadable)?;
    linux_case_file(&file)
}

#[cfg(target_os = "linux")]
fn linux_case_file(file: &File) -> Result<VolumeCase, RestoreFault> {
    use std::os::unix::io::AsRawFd;
    let mut stat = unsafe { std::mem::zeroed::<libc::statfs>() };
    if unsafe { libc::fstatfs(file.as_raw_fd(), &mut stat) } != 0 {
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
        return Ok(match ext_casefold_file(file) {
            Some(false) => VolumeCase::Bytes,
            Some(true) | None => VolumeCase::Unicode,
        });
    }
    if magic == FUSE {
        let path = path_of_handle(file)?;
        let kind = mount_fstype(&path).unwrap_or_default();
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
fn ext_casefold_file(file: &File) -> Option<bool> {
    use std::os::fd::AsRawFd;
    const FS_IOC_GETFLAGS: libc::Ioctl = 0x8008_6601;
    const FS_CASEFOLD_FL: libc::c_long = 0x4000_0000;
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
    fstype_at_mount(&text, target)
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

#[cfg(test)]
fn mkdir_exclusive(path: &Path) -> std::io::Result<()> {
    // Private, and new. `open_beneath` accepts this directory as its root and
    // refuses one that already existed.
    create_directory_exclusive(path)
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
fn an_existing_directory_is_not_a_new_private_directory() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("once");
    mkdir_exclusive(&path).unwrap();
    fs::write(path.join("sentinel"), b"keep").unwrap();
    let error = mkdir_exclusive(&path).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read(path.join("sentinel")).unwrap(), b"keep");
}

#[cfg(unix)]
#[test]
fn a_symlink_parent_is_not_a_staging_directory() {
    let root = tempfile::tempdir().unwrap();
    let partial = root.path().join("partial");
    mkdir_exclusive(&partial).unwrap();
    let outside = root.path().join("outside");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, partial.join("records")).unwrap();
    let mut staging = PartialTree {
        directory: open_directory(&partial, false).unwrap(),
        parent: open_directory(root.path(), false).unwrap(),
        partial_name: OsString::from("partial"),
        quarantine_name: OsString::from("quarantine"),
        armed: false,
    };
    assert_eq!(
        staging.write("records/note", b"one").unwrap_err(),
        RestoreFault::Unreadable
    );
    assert!(!outside.join("note").exists());
    assert!(partial
        .join("records")
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());
}

#[cfg(unix)]
#[test]
fn a_dangling_symlink_is_removed_without_creating_its_target() {
    let root = tempfile::tempdir().unwrap();
    let outside = root.path().join("outside");
    let link = root.path().join("session.partial");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    DirectoryQuarantine.remove_file(&link).unwrap();
    assert!(!outside.exists());
    assert!(fs::symlink_metadata(&link).is_err());
}

#[test]
fn a_different_published_directory_is_not_removed() {
    let root = tempfile::tempdir().unwrap();
    let quarantine = root.path().join("q");
    let live = root.path().join("live");
    fs::create_dir(&live).unwrap();
    let quarantine = DirectoryQuarantine.anchor(&quarantine).unwrap();
    let live = DirectoryQuarantine.anchor(&live).unwrap();
    let mut staging = DirectoryQuarantine.begin(&quarantine, &live).unwrap();
    staging.write("records/note", b"one").unwrap();
    let published = staging.finish("sealed\n").unwrap();
    let saved = quarantine.with_extension("saved");
    fs::rename(&quarantine, &saved).unwrap();
    fs::create_dir(&quarantine).unwrap();
    fs::write(quarantine.join("sentinel"), b"keep").unwrap();
    assert_eq!(
        DirectoryQuarantine
            .remove_known(&quarantine, &published)
            .unwrap_err(),
        RestoreFault::Tamper
    );
    assert_eq!(fs::read(quarantine.join("sentinel")).unwrap(), b"keep");
    DirectoryQuarantine
        .remove_known(&saved, &published)
        .unwrap();
    assert!(!saved.exists());
}

#[test]
fn a_changed_sealed_copy_is_not_promoted() {
    let root = tempfile::tempdir().unwrap();
    let dest = root.path().join("sealed");
    fs::create_dir(&dest).unwrap();
    fs::write(dest.join("note"), b"one").unwrap();
    fs::write(dest.join("journal.txt"), b"journal").unwrap();
    let created = open_directory(&dest, false).unwrap();
    let created_id = tree_id_of(&created).unwrap();
    let loaded = LoadedTree {
        journal: "journal".into(),
        files: Vec::new(),
        extras: Vec::new(),
        helpers: Vec::new(),
        identity: created_id,
        content: content_token(&dest).unwrap(),
    };
    fs::write(dest.join("note"), b"two").unwrap();
    assert_eq!(
        copy_still_sealed(&dest, &created, &created_id, &loaded).unwrap_err(),
        RestoreFault::Tamper
    );
    assert!(dest.join("note").is_file());
}

#[test]
fn a_replaced_sealed_directory_is_not_published() {
    let root = tempfile::tempdir().unwrap();
    let dest = root.path().join("sealed");
    fs::create_dir(&dest).unwrap();
    fs::write(dest.join("note"), b"one").unwrap();
    fs::write(dest.join("journal.txt"), b"journal").unwrap();
    let created = open_directory(&dest, false).unwrap();
    let created_id = tree_id_of(&created).unwrap();
    let loaded = LoadedTree {
        journal: "journal".into(),
        files: Vec::new(),
        extras: Vec::new(),
        helpers: Vec::new(),
        identity: created_id,
        content: content_token(&dest).unwrap(),
    };
    let aside = root.path().join("aside");
    fs::rename(&dest, &aside).unwrap();
    fs::create_dir(&dest).unwrap();
    fs::write(dest.join("sentinel"), b"keep").unwrap();
    assert_eq!(
        copy_still_sealed(&dest, &created, &created_id, &loaded).unwrap_err(),
        RestoreFault::Tamper
    );
    if tree_id(&dest).ok().as_ref() == Some(&created_id) {
        let _ = remove_same(&dest, &created_id);
    }
    assert_eq!(fs::read(dest.join("sentinel")).unwrap(), b"keep");
    assert_eq!(fs::read(aside.join("note")).unwrap(), b"one");
}

#[test]
fn a_renamed_directory_is_removed_by_its_handle() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("tree");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("note"), b"one").unwrap();
    let opened = open_directory(&dir, false).unwrap();
    let identity = tree_id_of(&opened).unwrap();
    let aside = root.path().join("aside");
    fs::rename(&dir, &aside).unwrap();
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("sentinel"), b"keep").unwrap();
    remove_opened(&opened, &identity).unwrap();
    assert_eq!(fs::read(dir.join("sentinel")).unwrap(), b"keep");
    assert!(!aside.exists());
}

#[test]
fn a_source_that_remains_loses_its_readiness_marker() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("quarantine");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("note"), b"one").unwrap();
    fs::write(dir.join("accept-ready"), b"stage-1\n").unwrap();
    let identity = tree_id(&dir).unwrap();
    let saved = dir.with_extension("saved");
    fs::rename(&dir, &saved).unwrap();
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("sentinel"), b"keep").unwrap();
    fs::write(dir.join("accept-ready"), b"stage-1\n").unwrap();
    retire_source(&dir, &identity).unwrap();
    assert_eq!(fs::read(dir.join("sentinel")).unwrap(), b"keep");
    assert_eq!(fs::read(dir.join("accept-ready")).unwrap(), b"stage-1\n");
    retire_source(&saved, &identity).unwrap();
    assert!(!saved.exists());
}

#[test]
fn a_remaining_source_is_not_left_acceptable() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("quarantine");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("note"), b"one").unwrap();
    fs::write(dir.join("accept-ready"), b"stage-1\n").unwrap();
    let identity = tree_id(&dir).unwrap();
    retire_source_after(&dir, &identity, |_, _| Err(RestoreFault::Unreadable)).unwrap();
    assert_eq!(fs::read(dir.join("note")).unwrap(), b"one");
    assert!(read_marker_id(&ready_path(&dir)).unwrap().is_none());
}

#[test]
fn the_sealed_copy_is_created_beside_live() {
    let root = tempfile::tempdir().unwrap();
    let live = root.path().join("live");
    let sealed = sealed_dest(&live).unwrap();
    assert_eq!(sealed.path.parent(), Some(root.path()));
    let name = sealed.path.file_name().unwrap().to_string_lossy();
    assert!(name.starts_with("live.sealed-"));
    assert!(directory_is_empty(&sealed.path).unwrap());
}

#[test]
fn a_credential_body_must_name_the_manifest_identity() {
    let fence = DeletionFence::credential(
        CredentialKind::Device,
        "credential-a",
        Fingerprint::of(b"revoked"),
    )
    .unwrap();
    let mismatched = br#"{"id":"credential-b","kind":"device"}"#;
    assert_eq!(
        omit_record(
            StoreKind::DeviceCredentials,
            "credential-a",
            mismatched,
            std::slice::from_ref(&fence)
        )
        .unwrap_err(),
        RestoreFault::Unreadable
    );
    let matched = br#"{"id":"credential-a","kind":"device"}"#;
    assert!(omit_record(
        StoreKind::DeviceCredentials,
        "credential-a",
        matched,
        std::slice::from_ref(&fence)
    )
    .unwrap());
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
    let parent = open_ancestor(root.path()).unwrap();
    let probe = probe_of(&parent).unwrap();
    let (claimed, _name) =
        claim_partial_locked(&parent, OsStr::new("q"), &live, probe, probe).unwrap();
    let claimed = path_of_handle(&claimed).unwrap();
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
    let anchored = DirectoryQuarantine.anchor(&tree).unwrap();
    assert!(DirectoryQuarantine.ready(&anchored).unwrap().is_none());
    DirectoryQuarantine
        .mark_ready(&anchored, "stage-1")
        .unwrap();
    assert_eq!(
        DirectoryQuarantine.ready(&anchored).unwrap().as_deref(),
        Some("stage-1")
    );
    assert!(!tree.join("accept-ready.partial").exists());
    retire_marker(&tree.join("accept-ready")).unwrap();
    assert!(DirectoryQuarantine.ready(&anchored).unwrap().is_none());
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
    let anchored = DirectoryQuarantine.anchor(&tree).unwrap();
    assert!(DirectoryQuarantine.ready(&anchored).unwrap().is_none());
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

#[cfg(unix)]
#[test]
fn a_renamed_ancestor_is_not_where_later_effects_land() {
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join("parent");
    fs::create_dir(&parent).unwrap();
    let live = parent.join("live");
    let anchored = DirectoryQuarantine.anchor(&live).unwrap();
    let other = DirectoryQuarantine
        .anchor(&root.path().join("other"))
        .unwrap();
    let aside = root.path().join("aside");
    fs::rename(&parent, &aside).unwrap();
    let elsewhere = root.path().join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &parent).unwrap();
    let _hold = DirectoryQuarantine.hold(&anchored).unwrap();
    assert!(aside.join("live.restore-lock").is_file());
    assert!(!elsewhere.join("live.restore-lock").exists());
    let staging = DirectoryQuarantine.begin(&anchored, &other).unwrap();
    let partial = fs::read_dir(&aside).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("live.partial-")
    });
    assert!(partial);
    assert!(fs::read_dir(&elsewhere).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("partial-")
    }));
    drop(staging);
}

#[cfg(test)]
fn one_conversation(id: &str, payload: &[u8]) -> (Vec<u8>, Fingerprint) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.sqlite3");
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
    let incarnation = [1u8; 16];
    let tail = [0u8, 0, 0, 0, 0, 0, 0, 1];
    database
        .execute(
            "INSERT INTO event_streams(stream_key, public_id, incarnation, floor, tail)
             VALUES (1, ?1, ?2, ?3, ?3)",
            nessa_local_database::rusqlite::params![id, incarnation.as_slice(), tail.as_slice()],
        )
        .unwrap();
    database
        .execute(
            "INSERT INTO event_stream_names(public_id, latest_incarnation, active_stream_key)
             VALUES (?1, ?2, 1)",
            nessa_local_database::rusqlite::params![id, incarnation.as_slice()],
        )
        .unwrap();
    database
        .execute(
            "INSERT INTO event_records(stream_key, offset, event_id, schema_id, schema_version, payload)
             VALUES (1, ?1, 'event', 'nessa.record', 1, ?2)",
            nessa_local_database::rusqlite::params![tail.as_slice(), payload],
        )
        .unwrap();
    drop(database);
    let digest = DirectoryQuarantine.session_cut(&path, id).unwrap().digest;
    (fs::read(path).unwrap(), digest)
}

#[test]
fn a_write_through_the_temp_inode_does_not_change_the_published_file() {
    let root = tempfile::tempdir().unwrap();
    let live = root.path().join("live");
    mkdir_exclusive(&live).unwrap();
    let anchored = DirectoryQuarantine.anchor(&live).unwrap();
    let relative = "sessions/conv.sqlite3";
    let (bytes, digest) = one_conversation("conv", b"verified-bytes");
    let temp = DirectoryQuarantine
        .session_temp(&anchored, relative)
        .unwrap();
    fs::write(&temp, &bytes).unwrap();
    let mut writer = fs::OpenOptions::new().write(true).open(&temp).unwrap();
    DirectoryQuarantine
        .publish_verified(&anchored, &temp, relative, "conv", &digest)
        .unwrap();
    writer.write_all(b"tampered-bytes!!").unwrap();
    writer.sync_all().unwrap();
    assert_eq!(fs::read(live.join(relative)).unwrap(), bytes);
}

#[cfg(unix)]
#[test]
fn a_held_file_is_not_a_directory_entry_until_publication() {
    let root = tempfile::tempdir().unwrap();
    let parent = open_directory(root.path(), false).unwrap();
    let mut held = create_unpublished(
        &parent,
        OsStr::new("note"),
        "note",
        false,
        RestoreFault::Tamper,
    )
    .unwrap();
    held.store(b"sealed").unwrap();
    assert!(list_names(&parent).unwrap().is_empty());
    fs::write(root.path().join("note"), b"tampered").unwrap();
    assert_eq!(held.read_back().unwrap(), b"sealed");
    assert_eq!(
        held.publish_in(RestoreFault::Tamper).unwrap_err(),
        RestoreFault::Tamper
    );
    fs::remove_file(root.path().join("note")).unwrap();
    held.publish_in(RestoreFault::Tamper).unwrap();
    assert_eq!(fs::read(root.path().join("note")).unwrap(), b"sealed");
}

#[cfg(target_os = "linux")]
#[test]
fn a_session_cut_reads_the_unnamed_inode() {
    let root = tempfile::tempdir().unwrap();
    let parent = open_directory(root.path(), false).unwrap();
    let (bytes, digest) = one_conversation("conv", b"verified-bytes");
    let mut held = create_unpublished(
        &parent,
        OsStr::new("conv.sqlite3"),
        "conv.sqlite3",
        true,
        RestoreFault::SessionDiffers,
    )
    .unwrap();
    held.store(&bytes).unwrap();
    held.sync_file().unwrap();
    assert!(list_names(&parent).unwrap().is_empty());
    let stream = inspect_open_file(held.file_mut().unwrap(), "conv").unwrap();
    assert_eq!(Fingerprint::from_digest(stream.digest), digest);
    assert!(list_names(&parent).unwrap().is_empty());
    held.publish_in(RestoreFault::SessionDiffers).unwrap();
    assert_eq!(fs::read(root.path().join("conv.sqlite3")).unwrap(), bytes);
    assert!(list_names(&parent)
        .unwrap()
        .iter()
        .all(|name| !name.to_string_lossy().contains("verified-")));
}

#[cfg(windows)]
#[test]
fn a_held_file_rejects_a_second_writer() {
    let root = tempfile::tempdir().unwrap();
    let parent = open_directory(root.path(), false).unwrap();
    let held = create_unpublished(
        &parent,
        OsStr::new("note"),
        "note",
        false,
        RestoreFault::Tamper,
    )
    .unwrap();
    let error = fs::OpenOptions::new()
        .write(true)
        .open(root.path().join("note"))
        .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(32));
    drop(held);
}

#[test]
fn a_different_byte_is_not_published() {
    let root = tempfile::tempdir().unwrap();
    let live = root.path().join("live");
    mkdir_exclusive(&live).unwrap();
    let anchored = DirectoryQuarantine.anchor(&live).unwrap();
    let relative = "sessions/conv.sqlite3";
    let (bytes, _) = one_conversation("conv", b"other-bytes");
    let (_, digest) = one_conversation("conv", b"verified-bytes");
    let temp = DirectoryQuarantine
        .session_temp(&anchored, relative)
        .unwrap();
    fs::write(&temp, &bytes).unwrap();
    let error = DirectoryQuarantine
        .publish_verified(&anchored, &temp, relative, "conv", &digest)
        .unwrap_err();
    assert_eq!(error, RestoreFault::SourceChanged);
    assert!(!live.join(relative).exists());
    assert!(fs::read_dir(live.join("sessions"))
        .unwrap()
        .all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("verified-")));
}

#[test]
fn a_renamed_source_is_retired_through_its_handle() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("quarantine");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("accept-ready"), b"stage-1\n").unwrap();
    fs::write(dir.join("note"), b"one").unwrap();
    let opened = open_directory(&dir, false).unwrap();
    let identity = tree_id_of(&opened).unwrap();
    let aside = root.path().join("aside");
    fs::rename(&dir, &aside).unwrap();
    retire_opened(&opened, &identity).unwrap();
    assert!(!aside.exists());
    assert!(!dir.exists());
}

#[test]
fn a_mount_point_with_a_space_is_that_filesystem() {
    let text = "\
50 1 0:45 / /mnt/My\\040Drive rw,relatime - fuse.ntfs-3g /dev/sda1 rw
1 0 8:1 / / rw,relatime - ext4 /dev/root rw
";
    assert_eq!(
        fstype_at_mount(text, "/mnt/My Drive").as_deref(),
        Some("fuse.ntfs-3g")
    );
    assert_eq!(decode_mount_field(r"/mnt/My\040Drive"), "/mnt/My Drive");
    assert_eq!(decode_mount_field(r"\011\012\134"), "\t\n\\");
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
