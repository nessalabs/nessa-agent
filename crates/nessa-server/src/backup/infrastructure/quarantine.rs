//! Quarantine directories, sealed inventory files, and deferred session copies.
//!
//! The application decides what may be kept. This module reads JSON records,
//! writes the partial tree, and streams one oversized conversation.

#[cfg(windows)]
#[path = "quarantine_closed_windows.rs"]
mod closed_windows;

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

#[cfg(test)]
use nessa_local_storage::create_directory_exclusive;
#[cfg(windows)]
#[path = "quarantine_windows.rs"]
mod windows_backend;

#[cfg(test)]
use nessa_local_storage::publish_new;
use nessa_local_storage::{create_directory, OpenMode};
use serde_json::{json, Value};

use sha2::Digest;

use crate::backup::application::{
    current_journal_is_trusted, current_staging_id, AnchoredRoot, CutMaterializer,
    DecisionAuditError, DeferredSessionSource, ExportClock, FetchProgress, JournalFields,
    LoadedTree, MacKey, PreparedCut, Promoted, PublicationIds, PublishedSession, QuarantineLayout,
    RestoreAudit, RestoreEvidence, RestoreHold, SealFields, SealedInventoryReader, SessionAttempt,
    SessionCut, SessionSourceError, StagedTreeOwner, Staging, StagingIds, TreeId,
    VerificationFormat, VerificationLimits, VerificationSource,
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
use super::snapshot::inspect_open_file;
use super::snapshot::{
    inspect_conversation, materialize_into_file, without_fenced_conversations, MaterializeError,
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
            files: Vec::new(),
            directories: Vec::new(),
            parent_spelling: quarantine
                .path()
                .parent()
                .ok_or(RestoreFault::Paths)?
                .to_path_buf(),
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
        generation: &str,
    ) -> Result<Promoted, RestoreFault> {
        promote_tree(quarantine, live, loaded, generation, retire_source_marker)
    }

    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault> {
        fs::read(path).map_err(|_| RestoreFault::Unreadable)
    }

    fn begin_session(
        &self,
        root: &AnchoredRoot,
        relative: &str,
    ) -> Result<Box<dyn SessionAttempt + '_>, RestoreFault> {
        let root_directory = open_root_dir(root)?;
        #[cfg(windows)]
        let mut session_guard = begin_session_directory(root, &root_directory, relative)?;
        let setup = |directory: File| {
            let name = relative_leaf(relative)?;
            let mut publication = name.clone();
            publication.push(".publication");
            if name_exists_at(&directory, &publication)? {
                return Err(RestoreFault::Retained);
            }
            let mut temporary = name.clone();
            temporary.push(".partial");
            // An existing partial is not this attempt's inode.
            if name_exists_at(&directory, &temporary)? {
                return Err(RestoreFault::Retained);
            }
            #[cfg(any(target_os = "macos", windows))]
            let (temporary_parent, temporary) = {
                let (parent, mut scratch) = parent_of_root(root)?;
                scratch.push(format!(
                    ".session-source-{}-{}",
                    std::process::id(),
                    ATTEMPT.fetch_add(1, Ordering::Relaxed)
                ));
                (parent, scratch)
            };
            #[cfg(not(any(target_os = "macos", windows)))]
            let temporary_parent = duplicate(&directory)?;
            let temp = create_unpublished(
                &temporary_parent,
                &temporary,
                relative,
                true,
                RestoreFault::Retained,
            )?;
            Ok((directory, name, temp))
        };
        #[cfg(windows)]
        let (directory, name, temp) =
            session_guard.prepare(|directory| setup(duplicate(directory)?))?;
        #[cfg(not(windows))]
        let (directory, name, temp) = setup(directory_under(&root_directory, relative)?)?;
        Ok(Box::new(DirectorySessionAttempt {
            root: root_directory,
            root_spelling: root.path().to_path_buf(),
            directory,
            name,
            relative: relative.to_owned(),
            temp,
            output: None,
            sidecar: None,
            namespace: None,
            #[cfg(windows)]
            session_guard,
            expected: None,
        }))
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

    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault> {
        let directory = open_root_dir(root)?;
        match kind_at(&directory, OsStr::new("accept-ready"))? {
            EntryKind::File => {
                read_marker_bytes(&read_bytes_at(&directory, OsStr::new("accept-ready"))?)
            }
            _ => Ok(None),
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

    fn session_cut_relative(
        &self,
        root: &AnchoredRoot,
        relative: &str,
        conversation_id: &str,
    ) -> Result<SessionCut, RestoreFault> {
        let directory = directory_for(root, relative)?;
        let leaf = relative_leaf(relative)?;
        let mut file = open_file_at(&directory, &leaf, OpenMode::Read)?;
        held_session_cut(&mut file, conversation_id)
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

#[cfg(windows)]
fn begin_session_directory(
    root: &AnchoredRoot,
    directory: &File,
    relative: &str,
) -> Result<windows_backend::SessionDirectory, RestoreFault> {
    if !relative_path_is_safe(relative) {
        return Err(RestoreFault::Tamper);
    }
    let parent = Path::new(relative).parent().ok_or(RestoreFault::Tamper)?;
    let mut components = parent.components().collect::<Vec<_>>();
    let Some(last) = components.pop() else {
        let (parent, name) = parent_of_root(root)?;
        return windows_backend::SessionDirectory::begin(&parent, &name);
    };
    let std::path::Component::Normal(name) = last else {
        return Err(RestoreFault::Tamper);
    };
    let mut held = duplicate(directory)?;
    for component in components {
        let std::path::Component::Normal(part) = component else {
            return Err(RestoreFault::Tamper);
        };
        held = ensure_dir_at(&held, part)?;
    }
    windows_backend::SessionDirectory::begin(&held, name)
}

/// Inspect the retained inode, even if its former name or ancestor is replaced.
fn held_session_cut(file: &mut File, conversation_id: &str) -> Result<SessionCut, RestoreFault> {
    let stream = inspect_open_file(file, conversation_id).map_err(|_| RestoreFault::Tamper)?;
    Ok(SessionCut {
        size: stream.size,
        digest: Fingerprint::from_digest(stream.digest),
    })
}

struct DirectorySessionAttempt {
    root: File,
    root_spelling: PathBuf,
    directory: File,
    name: OsString,
    relative: String,
    temp: Unpublished,
    output: Option<Unpublished>,
    sidecar: Option<Unpublished>,
    namespace: Option<ProtectedDirectories>,
    #[cfg(windows)]
    session_guard: windows_backend::SessionDirectory,
    expected: Option<(String, Fingerprint)>,
}

impl SessionAttempt for DirectorySessionAttempt {
    fn destination(&mut self) -> Result<File, RestoreFault> {
        duplicate(self.temp.file.as_ref().ok_or(RestoreFault::Unreadable)?)
    }

    fn inspect(&self, conversation_id: &str) -> Result<SessionCut, RestoreFault> {
        let mut file = self
            .temp
            .file
            .as_ref()
            .ok_or(RestoreFault::Unreadable)?
            .try_clone()
            .map_err(|_| RestoreFault::Unreadable)?;
        let stream =
            inspect_open_file(&mut file, conversation_id).map_err(|_| RestoreFault::Tamper)?;
        Ok(SessionCut {
            size: stream.size,
            digest: Fingerprint::from_digest(stream.digest),
        })
    }

    fn publish(&mut self, conversation_id: &str, digest: &Fingerprint) -> Result<(), RestoreFault> {
        let mut input = self
            .temp
            .file
            .as_ref()
            .ok_or(RestoreFault::Unreadable)?
            .try_clone()
            .map_err(|_| RestoreFault::Unreadable)?;
        input
            .seek(SeekFrom::Start(0))
            .map_err(|_| RestoreFault::Unreadable)?;
        self.output = Some(create_unpublished(
            &self.directory,
            &self.name,
            &self.relative,
            true,
            RestoreFault::SessionDiffers,
        )?);
        let output = self.output.as_mut().ok_or(RestoreFault::Unreadable)?;
        let mut buffer = [0u8; 8192];
        loop {
            let count = input
                .read(&mut buffer)
                .map_err(|_| RestoreFault::Unreadable)?;
            if count == 0 {
                break;
            }
            output.store(&buffer[..count])?;
        }
        output.sync_file()?;
        if !unpublished_matches(output, conversation_id, digest)? {
            return Err(RestoreFault::SourceChanged);
        }
        output.publish_in(RestoreFault::SessionDiffers)?;
        self.expected = Some((conversation_id.to_owned(), *digest));
        sync_dir(&self.directory)?;
        // Source bytes are retired by inode. On macOS its named scratch is
        // outside the restore root and remains empty; it is not a session helper.
        empty_owned_file(self.temp.file.as_ref().ok_or(RestoreFault::Retained)?)?;
        Ok(())
    }

    fn note_publication(&mut self, publication: &str) -> Result<PublishedSession, RestoreFault> {
        if publication.is_empty() || publication.bytes().any(|byte| byte <= b' ') {
            return Err(RestoreFault::Unreadable);
        }
        let output = self.output.as_ref().ok_or(RestoreFault::Unreadable)?;
        if !output.name_is_owned()? {
            return Err(RestoreFault::Tamper);
        }
        let mut name = self.name.clone();
        name.push(".publication");
        self.sidecar = Some(create_unpublished(
            &self.directory,
            &name,
            "publication",
            true,
            RestoreFault::SessionDiffers,
        )?);
        let sidecar = self.sidecar.as_mut().ok_or(RestoreFault::Unreadable)?;
        sidecar.store(publication.as_bytes())?;
        sidecar.store(b"\n")?;
        sidecar.sync_file()?;
        sidecar.publish_in(RestoreFault::SessionDiffers)?;
        #[cfg(windows)]
        self.session_guard.seal()?;
        #[cfg(not(windows))]
        {
            self.namespace = Some(ProtectedDirectories::seal(&self.directory)?);
        }
        if path_of_handle(&self.root)? != self.root_spelling
            || tree_id_of(&descend(&self.root, &self.relative, false)?)?
                != tree_id_of(&self.directory)?
        {
            return Err(RestoreFault::Tamper);
        }
        if !self
            .output
            .as_ref()
            .ok_or(RestoreFault::Unreadable)?
            .name_is_owned()?
            || !self
                .sidecar
                .as_ref()
                .ok_or(RestoreFault::Unreadable)?
                .name_is_owned()?
        {
            return Err(RestoreFault::Tamper);
        }
        let (conversation, digest) = self.expected.as_ref().ok_or(RestoreFault::Unreadable)?;
        if !unpublished_matches(
            self.output.as_mut().ok_or(RestoreFault::Unreadable)?,
            conversation,
            digest,
        )? {
            return Err(RestoreFault::SourceChanged);
        }
        Ok(PublishedSession {
            durable: directory_publication_is_durable(&self.directory),
        })
    }

    fn release(&mut self) -> Result<(), RestoreFault> {
        if let Some(file) = self.output.as_mut() {
            file.release()?;
        }
        if let Some(file) = self.sidecar.as_mut() {
            file.release()?;
        }
        if let Some(namespace) = self.namespace.as_mut() {
            if !namespace.release() {
                return Err(RestoreFault::Retained);
            }
        }
        #[cfg(windows)]
        self.session_guard.release()?;
        Ok(())
    }

    fn retire(&mut self) -> Result<(), RestoreFault> {
        // Give each retained object a cleanup attempt even when another fails.
        let temp = self.temp.retire();
        let output = self.output.as_mut().map(Unpublished::retire).transpose();
        let sidecar = self.sidecar.as_mut().map(Unpublished::retire).transpose();
        #[cfg(windows)]
        let restored = self.session_guard.release().is_ok();
        #[cfg(not(windows))]
        let restored = true;
        if temp.is_err() || output.is_err() || sidecar.is_err() || !restored {
            Err(RestoreFault::Retained)
        } else {
            Ok(())
        }
    }
}

struct PublicationOwner {
    files: Vec<Unpublished>,
    journal: Option<Unpublished>,
    directories: Vec<OwnedDirectory>,
    namespace: ProtectedDirectories,
    #[cfg(windows)]
    closed: Option<closed_windows::ClosedTree>,
}

impl PublicationOwner {
    fn prepare_rename(&mut self, root: &File) -> Result<(), RestoreFault> {
        #[cfg(windows)]
        {
            let closed = closed_windows::ClosedTree::capture_publication(
                root,
                &self.files,
                self.journal.as_ref().ok_or(RestoreFault::Retained)?,
                &self.directories,
            )?;
            self.files.clear();
            drop(self.journal.take());
            self.directories.clear();
            self.namespace.close_for_rename();
            self.closed = Some(closed);
        }
        #[cfg(not(windows))]
        {
            let _ = root;
            for directory in &self.directories {
                directory.check_binding()?;
            }
        }
        Ok(())
    }
    fn retire(&mut self, root: &File) -> RestoreFault {
        #[cfg(windows)]
        if let Some(closed) = &self.closed {
            let _ = closed.retire(root);
            return RestoreFault::Retained;
        }
        let _ = root;
        abandon_sealed(&mut self.files, self.journal.as_mut())
    }
    fn release(&mut self, root: &File) -> bool {
        #[cfg(windows)]
        if let Some(closed) = &self.closed {
            return closed.release(root);
        }
        let _ = root;
        let mut complete = true;
        for file in self.files.iter_mut().chain(self.journal.iter_mut()) {
            if file.release().is_err() {
                complete = false;
            }
        }
        if !self.namespace.release() {
            complete = false;
        }
        complete
    }
}

fn promote_tree(
    quarantine: &AnchoredRoot,
    live: &AnchoredRoot,
    loaded: &LoadedTree,
    generation: &str,
    retire: impl FnOnce(&File, &TreeId, &File) -> Result<(), RestoreFault>,
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
    let mut source_marker = open_file_at(&opened, OsStr::new("accept-ready"), OpenMode::Read)?;
    let marker_bytes_read = read_limited(&mut source_marker, MAX_NAME_BYTES as u64 + 1)?;
    if read_marker_bytes(&marker_bytes_read)?.as_deref() != Some(generation) {
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
    let mut owned_directories = Vec::new();
    let copied = match copy_at(&opened, &created, &mut held, &mut owned_directories) {
        Ok(token) => token,
        Err(_error) => {
            return Err(abandon_sealed(&mut held, None));
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
        Err(_error) => {
            return Err(abandon_sealed(&mut held, None));
        }
    };
    if journal.store(loaded.journal.as_bytes()).is_err() || journal.sync_file().is_err() {
        return Err(abandon_sealed(&mut held, Some(&mut journal)));
    }
    if copied != loaded.content || tree_id_of(&opened).ok().as_ref() != Some(&loaded.identity) {
        return Err(abandon_sealed(&mut held, Some(&mut journal)));
    }
    if let Err(_error) = held_still_sealed(&created, &created_id, loaded, &mut held, &mut journal) {
        return Err(abandon_sealed(&mut held, Some(&mut journal)));
    }
    let marker = (|| {
        source_marker
            .seek(SeekFrom::Start(0))
            .map_err(|_| RestoreFault::Unreadable)?;
        let bytes = read_limited(&mut source_marker, MAX_NAME_BYTES as u64 + 1)?;
        if read_marker_bytes(&bytes)?.as_deref() != Some(generation) {
            return Err(RestoreFault::Tamper);
        }
        held.push(create_unpublished(
            &created,
            OsStr::new("accept-ready"),
            "accept-ready",
            false,
            RestoreFault::Tamper,
        )?);
        let marker = held.last_mut().ok_or(RestoreFault::Retained)?;
        marker.store(&marker_bytes(generation)?)?;
        marker.sync_file()
    })();
    if marker.is_err() {
        return Err(abandon_sealed(&mut held, Some(&mut journal)));
    }
    // The link is the publication of each inode. The directory rename
    // then makes those names live. A descriptor opened on a predictable
    // copy name is not one of these inodes.
    if let Err(_error) = publish_held(&created, &mut held, &mut journal, sync_tree_at) {
        return Err(abandon_sealed(&mut held, Some(&mut journal)));
    }
    let mut namespace = match ProtectedDirectories::seal_owned(&created, &owned_directories) {
        Ok(guard) => guard,
        Err(_error) => {
            return Err(abandon_sealed(&mut held, Some(&mut journal)));
        }
    };
    if let Err(_error) = published_tree_matches(&created, loaded, &held, &journal) {
        namespace.release();
        return Err(abandon_sealed(&mut held, Some(&mut journal)));
    }
    let published = created_id;
    let mut publication = PublicationOwner {
        files: held,
        journal: Some(journal),
        directories: owned_directories,
        namespace,
        #[cfg(windows)]
        closed: None,
    };
    if publication.prepare_rename(&created).is_err() {
        return Err(publication.retire(&created));
    }
    // Native Windows rename requires every descendant handle closed. Unix
    // retains them; the same owner preserves identity and cleanup in both cases.
    if rename_owned_at(
        &created,
        &live_parent,
        &sealed_name,
        &live_parent,
        &live_name,
        RestoreFault::LiveOccupied,
    )
    .is_err()
    {
        return Err(publication.retire(&created));
    }
    let live_id = open_dir_at(&live_parent, &live_name)
        .ok()
        .and_then(|file| tree_id_of(&file).ok());
    let opened_id = tree_id_of(&created).ok();
    if live_id.as_ref() != Some(&published) || opened_id.as_ref() != Some(&published) {
        return Err(publication.retire(&created));
    }
    // The copy is live. Retire the source the open handle still names,
    // even when the original path is gone.
    // The rename already made the tree live. A parent that cannot be
    // synced does not put it back, and does not make the promotion a
    // refusal.
    let publication_restricted = !publication.release(&created);
    let mut promoted = finish_promotion(
        || retire(&opened, &loaded.identity, &source_marker),
        || {
            let live_synced = directory_publication_is_durable(&live_parent);
            let source_synced = directory_publication_is_durable(&source_parent);
            live_synced && source_synced
        },
    );
    promoted.publication_restricted = publication_restricted;
    Ok(promoted)
}

fn abandon_sealed(files: &mut [Unpublished], journal: Option<&mut Unpublished>) -> RestoreFault {
    for file in files {
        let _ = file.retire();
    }
    if let Some(journal) = journal {
        let _ = journal.retire();
    }
    RestoreFault::Retained
}

fn empty_owned_file(file: &File) -> Result<(), RestoreFault> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| RestoreFault::Retained)?;
        if file.set_len(0).is_ok() {
            return file.sync_all().map_err(|_| RestoreFault::Retained);
        }
        #[cfg(target_os = "linux")]
        let path = {
            use std::os::unix::io::AsRawFd;
            PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()))
        };
        #[cfg(not(target_os = "linux"))]
        let path = path_of_handle(file)?;
        let reopened = fs::OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|_| RestoreFault::Retained)?;
        if tree_id_of(&reopened)? != tree_id_of(file)? {
            return Err(RestoreFault::Retained);
        }
        reopened
            .set_len(0)
            .and_then(|()| reopened.sync_all())
            .map_err(|_| RestoreFault::Retained)
    }
    #[cfg(windows)]
    {
        if file.set_len(0).is_ok() {
            return file.sync_all().map_err(|_| RestoreFault::Retained);
        }
        let identity = nessa_local_storage::NativeObjectId::capture(file)
            .map_err(|_| RestoreFault::Retained)?;
        let writable = nessa_local_storage::reacquire_exclusive_file(file, identity)
            .map_err(|_| RestoreFault::Retained)?;
        writable
            .set_len(0)
            .and_then(|()| writable.sync_all())
            .map_err(|_| RestoreFault::Retained)
    }
}

fn published_tree_matches(
    directory: &File,
    loaded: &LoadedTree,
    held: &[Unpublished],
    journal: &Unpublished,
) -> Result<(), RestoreFault> {
    for file in held.iter().chain(std::iter::once(journal)) {
        if !file.name_is_owned()? {
            return Err(RestoreFault::Tamper);
        }
    }
    if content_of(directory)? != loaded.content
        || read_file_at(directory, OsStr::new("journal.txt"))? != loaded.journal
    {
        return Err(RestoreFault::Tamper);
    }
    Ok(())
}

fn restrict_directory(directory: &File) -> Result<File, RestoreFault> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let retained = duplicate(directory)?;
        retained
            .set_permissions(fs::Permissions::from_mode(0o500))
            .map_err(|_| RestoreFault::Unreadable)?;
        Ok(retained)
    }
    #[cfg(windows)]
    {
        windows_backend::seal_copy_directory(directory)
    }
}

#[cfg(test)]
pub(crate) fn promote_with_failed_source_retirement(
    quarantine: &AnchoredRoot,
    live: &AnchoredRoot,
    loaded: &LoadedTree,
    generation: &str,
    attempts: &[std::sync::atomic::AtomicUsize; 2],
    durable: bool,
) -> Result<Promoted, RestoreFault> {
    let mut promoted = promote_tree(
        quarantine,
        live,
        loaded,
        generation,
        |file, identity, marker| {
            retire_source_marker_with(
                file,
                identity,
                marker,
                |_, _| {
                    attempts[0].fetch_add(1, Ordering::Relaxed);
                    Err(RestoreFault::Unreadable)
                },
                |_| {
                    attempts[1].fetch_add(1, Ordering::Relaxed);
                    Err(RestoreFault::Retained)
                },
            )
        },
    )?;
    promoted.durable &= durable;
    Ok(promoted)
}

/// A child directory created and retained by this copy, never rediscovered for
/// metadata changes through a mutable namespace.
struct OwnedDirectory {
    relative: String,
    parent: File,
    name: OsString,
    directory: File,
}

impl OwnedDirectory {
    fn check_binding(&self) -> Result<(), RestoreFault> {
        let named = open_dir_at(&self.parent, &self.name)?;
        if tree_id_of(&named)? != tree_id_of(&self.directory)? {
            return Err(RestoreFault::Tamper);
        }
        Ok(())
    }
}

/// Protect only explicitly retained directories while final bindings are checked.
struct ProtectedDirectories {
    directories: Vec<(File, u32)>,
    armed: bool,
}

impl ProtectedDirectories {
    #[cfg(any(not(windows), test))]
    fn seal(directory: &File) -> Result<Self, RestoreFault> {
        Self::seal_owned(directory, &[])
    }

    fn seal_owned(directory: &File, children: &[OwnedDirectory]) -> Result<Self, RestoreFault> {
        let mut guard = Self {
            directories: Vec::new(),
            armed: true,
        };
        guard.protect(directory)?;
        for child in children {
            guard.protect(&child.directory)?;
        }
        // Every containing directory is now sealed. A substituted child cannot
        // be adopted, chmodded, or accepted even when its content is identical.
        for child in children {
            child.check_binding()?;
        }
        Ok(guard)
    }

    fn protect(&mut self, directory: &File) -> Result<(), RestoreFault> {
        #[cfg(unix)]
        let original = {
            use std::os::unix::fs::PermissionsExt;
            directory
                .metadata()
                .map_err(|_| RestoreFault::Unreadable)?
                .permissions()
                .mode()
                & 0o7777
        };
        #[cfg(not(unix))]
        let original = 0;
        self.directories
            .push((restrict_directory(directory)?, original));
        Ok(())
    }

    #[cfg(windows)]
    fn close_for_rename(&mut self) {
        self.armed = false;
        self.directories.clear();
    }

    fn release(&mut self) -> bool {
        if !self.armed {
            return true;
        }
        let mut restored = true;
        for (directory, original) in self.directories.iter().rev() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if directory
                    .set_permissions(fs::Permissions::from_mode(*original))
                    .is_err()
                {
                    restored = false;
                }
            }
            #[cfg(windows)]
            {
                let _ = original;
                if windows_backend::release_copy_directory(directory).is_err() {
                    restored = false;
                }
            }
        }
        self.armed = false;
        restored
    }
}
impl Drop for ProtectedDirectories {
    fn drop(&mut self) {
        self.release();
    }
}

struct RootHold {
    _file: File,
    _parent: File,
}

impl RestoreHold for RootHold {}

/// The live rename already happened. Retirement and durability are separate
/// facts about that live copy, rather than pre-rename refusal errors.
fn finish_promotion(
    retire: impl FnOnce() -> Result<(), RestoreFault>,
    sync_parents: impl FnOnce() -> bool,
) -> Promoted {
    let source_retirement_failed = retire().is_err();
    Promoted {
        durable: sync_parents(),
        source_retirement_failed,
        publication_restricted: false,
    }
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

struct OwnedStageFile {
    relative: String,
    file: File,
    fingerprint: Fingerprint,
}

struct PartialTree {
    directory: File,
    parent: File,
    partial_name: OsString,
    quarantine_name: OsString,
    armed: bool,
    files: Vec<OwnedStageFile>,
    directories: Vec<OwnedDirectory>,
    parent_spelling: PathBuf,
}

struct OwnedStagedTree {
    directory: File,
    parent: File,
    name: OsString,
    identity: TreeId,
    files: Vec<OwnedStageFile>,
    marker: Option<Unpublished>,
    parent_spelling: PathBuf,
}

impl StagedTreeOwner for OwnedStagedTree {
    fn check_binding(&self) -> Result<(), RestoreFault> {
        if path_of_handle(&self.parent)? != self.parent_spelling {
            return Err(RestoreFault::Retained);
        }
        let named = open_dir_at(&self.parent, &self.name).map_err(|_| RestoreFault::Retained)?;
        if tree_id_of(&named)? != self.identity {
            return Err(RestoreFault::Retained);
        }
        Ok(())
    }

    fn mark_ready(&mut self, staging_id: &str) -> Result<(), RestoreFault> {
        self.mark_ready_with(staging_id, sync_dir)
    }

    fn retire(&mut self) -> Result<(), RestoreFault> {
        // A directory cannot be unlinked conditionally by inode on POSIX.
        // Keep its namespace, retire only handles created by this stage, and
        // report that named directory honestly rather than touching a lookup.
        if let Some(marker) = self.marker.as_mut() {
            let _ = marker.retire();
        }
        for file in &self.files {
            let _ = empty_owned_file(&file.file);
        }
        Err(RestoreFault::Retained)
    }
}

impl OwnedStagedTree {
    fn mark_ready_with(
        &mut self,
        staging_id: &str,
        sync: impl FnOnce(&File) -> Result<(), RestoreFault>,
    ) -> Result<(), RestoreFault> {
        self.check_binding()?;
        let bytes = marker_bytes(staging_id)?;
        self.marker = Some(create_unpublished(
            &self.directory,
            OsStr::new("accept-ready"),
            "accept-ready",
            false,
            RestoreFault::Retained,
        )?);
        // Own the inode before any write, link, sync, or binding check can fail.
        let marker = self.marker.as_mut().ok_or(RestoreFault::Retained)?;
        marker.store(&bytes)?;
        marker.sync_file()?;
        marker.publish_in(RestoreFault::Retained)?;
        sync(&self.directory)?;
        self.check_binding()?;
        if !self
            .marker
            .as_ref()
            .ok_or(RestoreFault::Retained)?
            .name_is_owned()?
        {
            return Err(RestoreFault::Retained);
        }
        Ok(())
    }
}

fn staged_tree_matches(
    root: &File,
    files: &[OwnedStageFile],
    directories: &[OwnedDirectory],
) -> Result<(), RestoreFault> {
    for entry in directories {
        entry.check_binding()?;
    }
    let mut expected = Vec::new();
    for entry in files {
        let directory = descend(root, &entry.relative, false)?;
        let name = relative_leaf(&entry.relative)?;
        let named = open_file_at(&directory, &name, OpenMode::Read)?;
        if tree_id_of(&named)? != tree_id_of(&entry.file)?
            || hash_at(&directory, &name)? != entry.fingerprint
        {
            return Err(RestoreFault::Retained);
        }
        if entry.relative != "journal.txt" && entry.relative != "accept-ready" {
            expected.push(format!("{} {}", entry.relative, entry.fingerprint.hex()));
        }
    }
    expected.sort();
    if content_of(root)? != Fingerprint::of(expected.join("\n").as_bytes()) {
        return Err(RestoreFault::Retained);
    }
    Ok(())
}

#[cfg(windows)]
fn verify_reacquired_stage(root: &File, files: &[OwnedStageFile]) -> Result<(), RestoreFault> {
    if staged_tree_matches(root, files, &[]).is_err() {
        for file in files {
            let _ = empty_owned_file(&file.file);
        }
        return Err(RestoreFault::Retained);
    }
    Ok(())
}

impl Staging for PartialTree {
    fn write(&mut self, relative: &str, bytes: &[u8]) -> Result<(), RestoreFault> {
        if !relative_path_is_safe(relative) {
            return Err(RestoreFault::Tamper);
        }
        let directory = owned_directory_under(&self.directory, relative, &mut self.directories)?;
        let leaf = relative_leaf(relative)?;
        // The partial directory is the anchor. A symlink planted at a parent
        // inside it is not a directory this attempt owns.
        let mut file = open_file_at(&directory, &leaf, OpenMode::CreateNew)?;
        self.files.push(OwnedStageFile {
            relative: relative.to_owned(),
            file: file.try_clone().map_err(|_| RestoreFault::Retained)?,
            fingerprint: Fingerprint::of(bytes),
        });
        file.write_all(bytes)
            .map_err(|_| RestoreFault::Unreadable)?;
        file.sync_all().map_err(|_| RestoreFault::Unreadable)?;
        Ok(())
    }

    fn finish(
        mut self: Box<Self>,
        journal: &str,
    ) -> Result<Box<dyn StagedTreeOwner>, RestoreFault> {
        let result = (|| {
            self.write("journal.txt", journal.as_bytes())?;
            sync_tree_at(&self.directory)?;
            let identity = tree_id_of(&self.directory)?;
            #[cfg(windows)]
            let mut namespace =
                ProtectedDirectories::seal_owned(&self.directory, &self.directories)?;
            staged_tree_matches(&self.directory, &self.files, &self.directories)?;
            #[cfg(windows)]
            let closed = {
                let closed = closed_windows::ClosedTree::capture_stage(
                    &self.directory,
                    &self.files,
                    &self.directories,
                )?;
                self.files.clear();
                self.directories.clear();
                namespace.close_for_rename();
                closed
            };
            if rename_owned_at(
                &self.directory,
                &self.parent,
                &self.partial_name,
                &self.parent,
                &self.quarantine_name,
                RestoreFault::QuarantineOccupied,
            )
            .is_err()
            {
                #[cfg(windows)]
                {
                    let _ = closed.retire(&self.directory);
                }
                return Err(RestoreFault::Retained);
            }
            #[cfg(windows)]
            {
                self.files = match closed.reacquire_stage(&self.directory) {
                    Ok(files) => files,
                    Err(error) => {
                        let _ = closed.retire(&self.directory);
                        return Err(error);
                    }
                };
                verify_reacquired_stage(&self.directory, &self.files)?;
                if !closed.release(&self.directory) {
                    for file in &self.files {
                        let _ = empty_owned_file(&file.file);
                    }
                    return Err(RestoreFault::Retained);
                }
            }
            let owner = OwnedStagedTree {
                directory: duplicate(&self.directory)?,
                parent: duplicate(&self.parent)?,
                name: self.quarantine_name.clone(),
                identity,
                files: std::mem::take(&mut self.files),
                marker: None,
                parent_spelling: self.parent_spelling.clone(),
            };
            let mut owner = owner;
            if owner.check_binding().is_err() || sync_dir(&self.parent).is_err() {
                let _ = owner.retire();
                return Err(RestoreFault::Retained);
            }
            Ok(Box::new(owner) as Box<dyn StagedTreeOwner>)
        })();
        match result {
            Ok(owner) => {
                self.armed = false;
                Ok(owner)
            }
            Err(_) => {
                for file in &self.files {
                    let _ = empty_owned_file(&file.file);
                }
                self.armed = false;
                Err(RestoreFault::Retained)
            }
        }
    }

    fn discard(mut self: Box<Self>) -> Result<(), RestoreFault> {
        self.armed = false;
        for file in &self.files {
            let _ = empty_owned_file(&file.file);
        }
        Err(RestoreFault::Retained)
    }
}

impl Drop for PartialTree {
    fn drop(&mut self) {
        if self.armed {
            for file in &self.files {
                let _ = empty_owned_file(&file.file);
            }
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

/// Rename `from` onto `to` only when `to` does not exist.
#[cfg(test)]
fn rename_new(from: &Path, to: &Path, occupied: RestoreFault) -> Result<(), RestoreFault> {
    match publish_new(from, to) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Err(occupied),
        Err(_) => Err(RestoreFault::Rename),
    }
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

/// Namespace removal of a source is unsupported without an exact native
/// directory capability. The captured marker is enough to make it unready.
fn retire_source_marker(
    directory: &File,
    identity: &TreeId,
    marker: &File,
) -> Result<(), RestoreFault> {
    retire_source_marker_with(
        directory,
        identity,
        marker,
        |_, _| Err(RestoreFault::Retained),
        empty_owned_file,
    )
}

fn retire_source_marker_with(
    directory: &File,
    identity: &TreeId,
    marker: &File,
    remove: impl FnOnce(&File, &TreeId) -> Result<(), RestoreFault>,
    retire_marker: impl FnOnce(&File) -> Result<(), RestoreFault>,
) -> Result<(), RestoreFault> {
    if tree_id_of(directory)? != *identity {
        return Err(RestoreFault::Retained);
    }
    match remove(directory, identity) {
        Ok(()) => Ok(()),
        Err(_) => retire_marker(marker),
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
        match create_directory_handle(parent, &partial_name, true) {
            Ok(directory) => {
                return Ok((directory, partial_name));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(RestoreFault::Retained),
        }
    }
    Err(RestoreFault::Unreadable)
}

#[cfg(test)]
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
#[cfg(any(test, windows))]
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
#[cfg(not(windows))]
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

fn create_directory_handle(parent: &File, name: &OsStr, root: bool) -> std::io::Result<File> {
    #[cfg(windows)]
    {
        windows_backend::create_directory(parent, name, root)
    }
    #[cfg(not(windows))]
    {
        let _ = root;
        mkdir_exclusive_at(parent, name)?;
        capture_created_directory(parent, name)
            .map_err(|_| std::io::Error::other("created directory capture refused"))
    }
}

fn owned_directory_under(
    root: &File,
    relative: &str,
    owned: &mut Vec<OwnedDirectory>,
) -> Result<File, RestoreFault> {
    if !relative_path_is_safe(relative) {
        return Err(RestoreFault::Tamper);
    }
    let mut directory = duplicate(root)?;
    let mut prefix = String::new();
    if let Some(parent) = Path::new(relative).parent() {
        for part in parent.components() {
            let std::path::Component::Normal(name) = part else {
                return Err(RestoreFault::Tamper);
            };
            prefix = child_relative(&prefix, name)?;
            if let Some(existing) = owned.iter().find(|entry| entry.relative == prefix) {
                existing.check_binding()?;
                directory = duplicate(&existing.directory)?;
            } else {
                if kind_at(&directory, name)? != EntryKind::Missing {
                    return Err(RestoreFault::Unreadable);
                }
                let created = create_directory_handle(&directory, name, false)
                    .map_err(|_| RestoreFault::Retained)?;
                owned.push(OwnedDirectory {
                    relative: prefix.clone(),
                    parent: duplicate(&directory)?,
                    name: name.to_os_string(),
                    directory: duplicate(&created)?,
                });
                directory = created;
            }
        }
    }
    Ok(directory)
}

/// Directory authority starts at descriptor capture, not at a claim of atomic
/// inode birth. Refuse a substituted nonempty or nonprivate object untouched.
#[cfg(not(windows))]
fn capture_created_directory(parent: &File, name: &OsStr) -> Result<File, RestoreFault> {
    let file = open_dir_at(parent, name).map_err(|_| RestoreFault::Retained)?;
    validate_captured_directory(&file)?;
    Ok(file)
}

#[cfg(not(windows))]
fn validate_captured_directory(directory: &File) -> Result<(), RestoreFault> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = directory.metadata().map_err(|_| RestoreFault::Retained)?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o7777 != 0o700
        {
            return Err(RestoreFault::Retained);
        }
        if !captured_directory_is_empty(directory)? {
            return Err(RestoreFault::Retained);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = directory;
    }
    Ok(())
}

#[cfg(unix)]
fn captured_directory_is_empty(directory: &File) -> Result<bool, RestoreFault> {
    use std::os::unix::io::AsRawFd;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            c".".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(RestoreFault::Retained);
    }
    let stream = unsafe { libc::fdopendir(fd) };
    if stream.is_null() {
        unsafe { libc::close(fd) };
        return Err(RestoreFault::Retained);
    }
    let result = loop {
        #[cfg(target_os = "linux")]
        unsafe {
            *libc::__errno_location() = 0;
        }
        #[cfg(target_os = "macos")]
        unsafe {
            *libc::__error() = 0;
        }
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            break if std::io::Error::last_os_error().raw_os_error() == Some(0) {
                Ok(true)
            } else {
                Err(RestoreFault::Retained)
            };
        }
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name != b"." && name != b".." {
            break Ok(false);
        }
    };
    unsafe { libc::closedir(stream) };
    result
}

fn ensure_dir_at(parent: &File, name: &OsStr) -> Result<File, RestoreFault> {
    match kind_at(parent, name)? {
        EntryKind::Missing => {
            create_directory_handle(parent, name, false).map_err(|_| RestoreFault::Retained)
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
    directories: &mut Vec<OwnedDirectory>,
) -> Result<Fingerprint, RestoreFault> {
    let mut lines = Vec::new();
    copy_entries(source, dest, "", &mut lines, held, directories)?;
    lines.sort();
    Ok(Fingerprint::of(lines.join("\n").as_bytes()))
}

fn copy_entries(
    source: &File,
    dest: &File,
    prefix: &str,
    lines: &mut Vec<String>,
    held: &mut Vec<Unpublished>,
    directories: &mut Vec<OwnedDirectory>,
) -> Result<(), RestoreFault> {
    for name in list_names(source)? {
        let relative = child_relative(prefix, &name)?;
        match kind_at(source, &name)? {
            EntryKind::Directory => {
                let child = open_dir_at(source, &name)?;
                let created = create_directory_handle(dest, &name, false)
                    .map_err(|_| RestoreFault::Retained)?;
                directories.push(OwnedDirectory {
                    relative: relative.clone(),
                    parent: duplicate(dest)?,
                    name: name.clone(),
                    directory: duplicate(&created)?,
                });
                copy_entries(&child, &created, &relative, lines, held, directories)?;
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
    let output = create_unpublished(dest, name, relative, false, RestoreFault::Tamper)?;
    held.push(output);
    let output = held.last_mut().ok_or(RestoreFault::Unreadable)?;
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
    Ok(fingerprint)
}

/// A file this attempt wrote and still holds.
///
/// Linux keeps the inode unnamed until [`Unpublished::publish_in`]. Other
/// Unix systems create its final name with mode0400 and retain the only trusted
/// writer. Windows retains a handle that does not share write access.
struct Unpublished {
    parent: File,
    name: OsString,
    relative: String,
    file: Option<File>,
    fingerprint: Fingerprint,
    /// Creation already installed `name` (all platforms except Linux).
    named: bool,
    /// The name is the one publication keeps. Drop must not remove it.
    published: bool,
    protected: bool,
}

impl Unpublished {
    /// Check whether the final name still denotes this retained inode.
    fn name_is_owned(&self) -> Result<bool, RestoreFault> {
        let held = self.file.as_ref().ok_or(RestoreFault::Unreadable)?;
        match kind_at(&self.parent, &self.name)? {
            EntryKind::Missing => Ok(false),
            EntryKind::File => Ok(tree_id_of(&open_file_at(
                &self.parent,
                &self.name,
                OpenMode::Read,
            )?)? == tree_id_of(held)?),
            _ => Ok(false),
        }
    }

    fn retire(&mut self) -> Result<(), RestoreFault> {
        let Some(file) = self.file.as_ref() else {
            return Ok(());
        };
        let emptied = empty_owned_file(file).is_ok();
        // Name removal cannot be made conditional on an inode atomically on
        // Unix. Leave the namespace alone; the retained bytes are now empty.
        // A replacement name is not this inode; emptying still retires any
        // descriptor retained by the source, even when it was renamed aside.
        if !emptied || self.published || self.named {
            Err(RestoreFault::Retained)
        } else {
            Ok(())
        }
    }

    fn protect(&mut self) -> Result<(), RestoreFault> {
        if self.protected {
            return Ok(());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file = self.file.as_ref().ok_or(RestoreFault::Unreadable)?;
            file.set_permissions(fs::Permissions::from_mode(0o400))
                .map_err(|_| RestoreFault::Unreadable)?;
            file.sync_all().map_err(|_| RestoreFault::Unreadable)?;
            #[cfg(target_os = "linux")]
            {
                use std::os::unix::io::AsRawFd;
                let readonly = File::open(format!("/proc/self/fd/{}", file.as_raw_fd()))
                    .map_err(|_| RestoreFault::Unreadable)?;
                if tree_id_of(&readonly)? != tree_id_of(file)? {
                    return Err(RestoreFault::Tamper);
                }
                self.file = Some(readonly);
            }
        }
        self.protected = true;
        Ok(())
    }

    fn release(&mut self) -> Result<(), RestoreFault> {
        if !self.protected {
            return Ok(());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            self.file
                .as_ref()
                .ok_or(RestoreFault::Unreadable)?
                .set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|_| RestoreFault::Unreadable)?;
        }
        #[cfg(windows)]
        {
            nessa_local_storage::release_file_writes(
                self.file.as_ref().ok_or(RestoreFault::Retained)?,
            )
            .map_err(|_| RestoreFault::Retained)?;
        }
        self.protected = false;
        Ok(())
    }
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

    /// Give `name` to this inode. On Windows the name already exists.
    fn publish_in(&mut self, occupied: RestoreFault) -> Result<(), RestoreFault> {
        self.protect()?;
        #[cfg(target_os = "linux")]
        {
            let file = self.file.as_ref().ok_or(RestoreFault::Unreadable)?;
            link_unpublished(file, &self.parent, &self.name, occupied)?;
        }
        #[cfg(not(target_os = "linux"))]
        {
            if !self.name_is_owned()? {
                return Err(occupied);
            }
        }
        self.published = true;
        Ok(())
    }
}

fn unpublished_matches(
    output: &mut Unpublished,
    conversation_id: &str,
    digest: &Fingerprint,
) -> Result<bool, RestoreFault> {
    Ok(matches!(
        inspect_open_file(output.file_mut()?,conversation_id),
        Ok(stream) if Fingerprint::from_digest(stream.digest)==*digest
    ))
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
        // The inode is read-only to every fresh namespace open from birth.
        // This private writable descriptor is never delegated to the source
        // when constructing a publication; publication always copies it.
        let created = open_at(
            &parent,
            name,
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o400,
        )?;
        if !created
            .metadata()
            .map_err(|_| RestoreFault::Retained)?
            .is_file()
        {
            return Err(RestoreFault::Retained);
        }
        created
    };
    #[cfg(windows)]
    let file = {
        let _ = private;
        windows_backend::protected_file(&parent, name, occupied)?
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
        named: !cfg!(target_os = "linux"),
        published: false,
        protected: false,
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

fn publish_held(
    directory: &File,
    held: &mut [Unpublished],
    journal: &mut Unpublished,
    sync: impl FnOnce(&File) -> Result<(), RestoreFault>,
) -> Result<(), RestoreFault> {
    for file in held.iter_mut() {
        file.publish_in(RestoreFault::Tamper)?;
    }
    journal.publish_in(RestoreFault::Tamper)?;
    // File sync happened while these inodes were unnamed on Unix. Persist
    // their directory entries after linking, including entries below the root,
    // before the directory rename can make the tree live.
    sync(directory)
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
/// A lifecycle acknowledgement is not proof of directory-entry durability.
/// Windows has no qualified directory-sync primitive in this adapter.
fn directory_publication_is_durable(directory: &File) -> bool {
    #[cfg(windows)]
    {
        let _ = directory;
        false
    }
    #[cfg(not(windows))]
    {
        sync_dir(directory).is_ok()
    }
}

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
        match create_directory_handle(parent, &name, true) {
            Ok(file) => {
                let identity = tree_id_of(&file).map_err(|_| RestoreFault::Retained)?;
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
            Err(_) => return Err(RestoreFault::Retained),
        }
    }
    Err(RestoreFault::Unreadable)
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
        windows_backend::open_directory(parent, name)
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
        windows_backend::open_file(parent, name, mode)
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

#[cfg(not(windows))]
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
        windows_backend::entry_kind(parent, name)
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
        windows_backend::names(directory)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = directory;
        Err(RestoreFault::Unreadable)
    }
}

#[cfg(test)]
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

fn rename_owned_at(
    source: &File,
    from_parent: &File,
    from: &OsStr,
    to_parent: &File,
    to: &OsStr,
    occupied: RestoreFault,
) -> Result<(), RestoreFault> {
    #[cfg(windows)]
    {
        let _ = (from_parent, from);
        windows_backend::rename_owned(source, to_parent, to, occupied)
    }
    #[cfg(not(windows))]
    {
        let _ = source;
        rename_exclusive_at(from_parent, from, to_parent, to, occupied)
    }
}

#[cfg(any(not(windows), test))]
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
        windows_backend::rename_entry(from_dir, from, to_dir, to, occupied)
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

#[cfg(all(windows, test))]
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
            "journalDigest": evidence.journal_digest,
            "rootGeneration": evidence.root_generation,
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
        let lines = read_restore_audit(&self.path)?;
        Ok(current_staging_id(
            lines.iter().map(restore_journal_fields),
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
        let _gate = self
            .gate
            .lock()
            .map_err(|_| DecisionAuditError::NotStored)?;
        let _file = exclusive(&self.directory).map_err(|_| DecisionAuditError::NotStored)?;
        let lines = read_restore_audit(&self.path)?;
        Ok(current_journal_is_trusted(
            lines.iter().map(restore_journal_fields),
            target,
            digest,
            staging_id,
        ))
    }
}

fn read_restore_audit(path: &Path) -> Result<Vec<Value>, DecisionAuditError> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(path).map_err(|_| DecisionAuditError::NotStored)?;
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).map_err(|_| DecisionAuditError::NotStored))
        .collect()
}

fn restore_audit_field<'a>(value: &'a Value, name: &str) -> &'a str {
    value.get(name).and_then(Value::as_str).unwrap_or("")
}

fn restore_journal_fields(value: &Value) -> JournalFields<'_> {
    JournalFields {
        seal: restore_seal_fields(value),
        journal_digest: restore_audit_field(value, "journalDigest"),
        root_generation: restore_audit_field(value, "rootGeneration"),
    }
}

fn restore_seal_fields(value: &Value) -> SealFields<'_> {
    SealFields {
        target: restore_audit_field(value, "target"),
        before: restore_audit_field(value, "before"),
        after: restore_audit_field(value, "after"),
        cause: restore_audit_field(value, "cause"),
        decision: restore_audit_field(value, "decision"),
        attestations: restore_audit_field(value, "attestations"),
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

impl DeferredSessionSource for SqliteDeferredSource {
    fn write_session(
        &self,
        conversation_id: &str,
        destination: &mut File,
        progress: &mut dyn FetchProgress,
    ) -> Result<(), SessionSourceError> {
        let mut report = |read, total| progress.report(read, total);
        materialize_into_file(&self.source, conversation_id, destination, &mut report)
            .map(|_| ())
            .map_err(materialize_error)
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
        files: Vec::new(),
        directories: Vec::new(),
        parent_spelling: root.path().to_path_buf(),
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
fn a_dangling_symlink_is_left_without_creating_its_target() {
    let root = tempfile::tempdir().unwrap();
    let live = root.path().join("live");
    mkdir_exclusive(&live).unwrap();
    fs::create_dir(live.join("sessions")).unwrap();
    let outside = root.path().join("outside");
    let link = live.join("sessions/conv.sqlite3.partial");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let anchored = DirectoryQuarantine.anchor(&live).unwrap();
    assert!(matches!(
        DirectoryQuarantine.begin_session(&anchored, "sessions/conv.sqlite3"),
        Err(RestoreFault::Retained)
    ));
    assert!(!outside.exists());
    assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
}

#[test]
fn a_different_published_directory_is_not_removed() {
    let root = tempfile::tempdir().unwrap();
    let q = root.path().join("q");
    let live = root.path().join("live");
    let qroot = DirectoryQuarantine.anchor(&q).unwrap();
    let liveroot = DirectoryQuarantine.anchor(&live).unwrap();
    let mut staging = DirectoryQuarantine.begin(&qroot, &liveroot).unwrap();
    staging.write("records/note", b"owned").unwrap();
    let mut owner = staging.finish("journal").unwrap();
    owner.mark_ready("stage-1").unwrap();
    let saved = root.path().join("saved");
    #[cfg(windows)]
    {
        let error = fs::rename(&q, &saved).unwrap_err();
        assert!(matches!(error.raw_os_error(), Some(5 | 32)));
        assert!(!saved.exists());
        assert_eq!(fs::read(q.join("records/note")).unwrap(), b"owned");
        assert_eq!(fs::read(q.join("journal.txt")).unwrap(), b"journal");
        assert_eq!(fs::read(q.join("accept-ready")).unwrap(), b"stage-1\n");
        owner.check_binding().unwrap();
        assert_eq!(owner.retire(), Err(RestoreFault::Retained));
        for relative in ["records/note", "journal.txt", "accept-ready"] {
            assert!(fs::read(q.join(relative)).unwrap().is_empty());
        }
    }
    #[cfg(not(windows))]
    {
        fs::rename(&q, &saved).unwrap();
        fs::create_dir(&q).unwrap();
        fs::write(q.join("sentinel"), b"keep").unwrap();
        assert_eq!(owner.check_binding(), Err(RestoreFault::Retained));
        assert_eq!(owner.retire(), Err(RestoreFault::Retained));
        assert_eq!(fs::read(q.join("sentinel")).unwrap(), b"keep");
        assert!(fs::read(saved.join("records/note")).unwrap().is_empty());
        assert!(fs::read(saved.join("journal.txt")).unwrap().is_empty());
        assert!(fs::read(saved.join("accept-ready")).unwrap().is_empty());
        assert!(saved.is_dir());
    }
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
    assert_eq!(fs::read(dest.join("sentinel")).unwrap(), b"keep");
    assert_eq!(fs::read(aside.join("note")).unwrap(), b"one");
}

#[test]
fn a_renamed_staged_ancestor_retires_only_its_owned_files() {
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join("parent");
    fs::create_dir(&parent).unwrap();
    let q = parent.join("q");
    let live = root.path().join("live");
    let qroot = DirectoryQuarantine.anchor(&q).unwrap();
    let liveroot = DirectoryQuarantine.anchor(&live).unwrap();
    let mut staging = DirectoryQuarantine.begin(&qroot, &liveroot).unwrap();
    staging.write("note", b"owned").unwrap();
    let mut owner = staging.finish("journal").unwrap();
    let aside = root.path().join("aside");
    #[cfg(windows)]
    {
        let error = fs::rename(&parent, &aside).unwrap_err();
        assert!(matches!(error.raw_os_error(), Some(5 | 32)));
        assert!(!aside.exists());
        assert_eq!(fs::read(q.join("note")).unwrap(), b"owned");
        assert_eq!(fs::read(q.join("journal.txt")).unwrap(), b"journal");
        owner.check_binding().unwrap();
        assert_eq!(owner.retire(), Err(RestoreFault::Retained));
        assert!(fs::read(q.join("note")).unwrap().is_empty());
        assert!(fs::read(q.join("journal.txt")).unwrap().is_empty());
    }
    #[cfg(not(windows))]
    {
        fs::rename(&parent, &aside).unwrap();
        fs::create_dir(&parent).unwrap();
        fs::create_dir(parent.join("q")).unwrap();
        fs::write(parent.join("q/sentinel"), b"other").unwrap();
        assert_eq!(owner.check_binding(), Err(RestoreFault::Retained));
        assert_eq!(owner.mark_ready("stage-1"), Err(RestoreFault::Retained));
        assert_eq!(owner.retire(), Err(RestoreFault::Retained));
        assert!(fs::read(aside.join("q/note")).unwrap().is_empty());
        assert_eq!(fs::read(parent.join("q/sentinel")).unwrap(), b"other");
    }
}

#[test]
fn a_source_that_remains_loses_its_readiness_marker() {
    let root = tempfile::tempdir().unwrap();
    let q = root.path().join("q");
    fs::create_dir(&q).unwrap();
    fs::write(q.join("note"), b"one").unwrap();
    fs::write(q.join("accept-ready"), b"stage-1\n").unwrap();
    let directory = open_directory(&q, false).unwrap();
    let marker = open_file_at(&directory, OsStr::new("accept-ready"), OpenMode::Read).unwrap();
    let id = tree_id_of(&directory).unwrap();
    let saved = root.path().join("saved");
    fs::rename(&q, &saved).unwrap();
    fs::create_dir(&q).unwrap();
    fs::write(q.join("accept-ready"), b"replacement\n").unwrap();
    retire_source_marker(&directory, &id, &marker).unwrap();
    assert!(fs::read(saved.join("accept-ready")).unwrap().is_empty());
    assert_eq!(fs::read(saved.join("note")).unwrap(), b"one");
    assert_eq!(fs::read(q.join("accept-ready")).unwrap(), b"replacement\n");
}

#[test]
fn a_remaining_source_is_not_left_acceptable() {
    let root = tempfile::tempdir().unwrap();
    let q = root.path().join("q");
    fs::create_dir(&q).unwrap();
    fs::write(q.join("note"), b"one").unwrap();
    fs::write(q.join("accept-ready"), b"stage-1\n").unwrap();
    let directory = open_directory(&q, false).unwrap();
    let marker = open_file_at(&directory, OsStr::new("accept-ready"), OpenMode::Read).unwrap();
    let id = tree_id_of(&directory).unwrap();
    retire_source_marker(&directory, &id, &marker).unwrap();
    assert_eq!(fs::read(q.join("note")).unwrap(), b"one");
    assert!(fs::read(q.join("accept-ready")).unwrap().is_empty());
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
            journal_digest: "",
            root_generation: "",
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
            journal_digest: "",
            root_generation: "",
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
            journal_digest: "",
            root_generation: "",
        })
        .unwrap();
    assert!(audit.sealed("quarantine", "digest-a").unwrap().is_none());
    audit
        .append(&RestoreEvidence {
            target: "quarantine",
            before: "verified",
            after: "ready",
            cause: "ready",
            initiator: "person",
            decision: "effect",
            policies: &[],
            attestations: "",
            journal_digest: "digest-a",
            root_generation: "stage-1",
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
            journal_digest: "",
            root_generation: "",
        })
        .unwrap();
    assert!(audit.sealed("quarantine", "digest-a").unwrap().is_none());
}

#[test]
fn journal_trust_survives_audit_reopen_and_tracks_the_latest_generation() {
    struct Clock;
    impl ExportClock for Clock {
        fn unix_ms(&self) -> u64 {
            1
        }
    }
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("audit");
    let reopen = || FileRestoreAudit::open(&directory, Clock).unwrap();
    let audit = reopen();
    let record = |before, after, cause, attestations, journal_digest, generation| {
        audit
            .append(&RestoreEvidence {
                target: "root",
                before,
                after,
                cause,
                initiator: "person",
                decision: "effect",
                policies: &[],
                attestations,
                journal_digest,
                root_generation: generation,
            })
            .unwrap();
    };
    let attestation = crate::backup::domain::seal_attestation("digest-a", "stage-1");
    record("absent", "verified", "sealed", &attestation, "", "stage-1");
    assert!(!reopen()
        .trusted_journal("root", "digest-a", Some("stage-1"))
        .unwrap());
    record("verified", "ready", "ready", "", "digest-a", "stage-1");
    assert!(reopen()
        .trusted_journal("root", "digest-a", Some("stage-1"))
        .unwrap());
    assert!(!reopen().trusted_journal("root", "digest-a", None).unwrap());
    record("verified", "spent", "spent", "stage-1", "", "stage-1");
    assert!(!reopen()
        .trusted_journal("root", "digest-a", Some("stage-1"))
        .unwrap());
    record(
        "verified",
        "promoting",
        "accepting",
        "",
        "digest-a",
        "stage-1",
    );
    record("verified", "live", "accepted", "", "digest-a", "stage-1");
    assert!(reopen()
        .trusted_journal("root", "digest-a", Some("stage-1"))
        .unwrap());
    assert!(!reopen().trusted_journal("root", "digest-a", None).unwrap());
    assert!(!reopen()
        .trusted_journal("other", "digest-a", Some("stage-1"))
        .unwrap());
    record(
        "verified",
        "promoting",
        "accepting",
        "",
        "digest-a",
        "stage-2",
    );
    assert!(!reopen()
        .trusted_journal("root", "digest-a", Some("stage-1"))
        .unwrap());
    assert!(!reopen()
        .trusted_journal("root", "digest-a", Some("stage-2"))
        .unwrap());
    record("verified", "live", "retained", "", "digest-a", "stage-2");
    assert!(reopen()
        .trusted_journal("root", "digest-a", Some("stage-2"))
        .unwrap());
    assert!(!reopen()
        .trusted_journal("root", "digest-a", Some("stage-1"))
        .unwrap());
    let text = fs::read_to_string(directory.join("restore.jsonl")).unwrap();
    assert!(text.contains("\"journalDigest\":\"digest-a\""));
    assert!(text.contains("\"rootGeneration\":\"stage-2\""));
}

#[cfg(test)]
fn owned_stage_for_test(path: &Path) -> OwnedStagedTree {
    mkdir_exclusive(path).unwrap();
    let mut note = fs::File::create(path.join("note")).unwrap();
    note.write_all(b"owned").unwrap();
    let directory = open_directory(path, false).unwrap();
    let identity = tree_id_of(&directory).unwrap();
    let parent = path.parent().unwrap();
    OwnedStagedTree {
        directory,
        parent: open_directory(parent, false).unwrap(),
        name: path.file_name().unwrap().to_owned(),
        identity,
        files: vec![OwnedStageFile {
            relative: "note".into(),
            file: note,
            fingerprint: Fingerprint::of(b"owned"),
        }],
        marker: None,
        parent_spelling: parent.to_path_buf(),
    }
}

#[test]
fn a_partial_readiness_file_is_not_the_marker() {
    let root = tempfile::tempdir().unwrap();
    let q = root.path().join("q");
    let mut owner = owned_stage_for_test(&q);
    fs::write(q.join("accept-ready.partial"), b"other occupant").unwrap();
    owner.mark_ready("stage-1").unwrap();
    assert_eq!(
        read_marker_bytes(&fs::read(q.join("accept-ready")).unwrap())
            .unwrap()
            .as_deref(),
        Some("stage-1")
    );
    assert_eq!(
        fs::read(q.join("accept-ready.partial")).unwrap(),
        b"other occupant"
    );
    assert_eq!(owner.retire(), Err(RestoreFault::Retained));
    assert!(fs::read(q.join("accept-ready")).unwrap().is_empty());
}

#[test]
fn a_readiness_directory_sync_failure_clears_the_marker() {
    let root = tempfile::tempdir().unwrap();
    let q = root.path().join("q");
    let mut owner = owned_stage_for_test(&q);
    assert_eq!(
        owner.mark_ready_with("stage-1", |_| Err(RestoreFault::Unreadable)),
        Err(RestoreFault::Unreadable)
    );
    assert_eq!(owner.retire(), Err(RestoreFault::Retained));
    assert!(fs::read(q.join("accept-ready")).unwrap().is_empty());
    assert!(q.exists());
}

#[test]
fn a_marker_that_cannot_be_cleared_is_retained() {
    let root = tempfile::tempdir().unwrap();
    let q = root.path().join("q");
    fs::create_dir(&q).unwrap();
    fs::write(q.join("accept-ready"), b"stage-1\n").unwrap();
    let directory = open_directory(&q, false).unwrap();
    let marker = open_file_at(&directory, OsStr::new("accept-ready"), OpenMode::Read).unwrap();
    let id = tree_id_of(&directory).unwrap();
    assert_eq!(
        retire_source_marker_with(
            &directory,
            &id,
            &marker,
            |_, _| Err(RestoreFault::Retained),
            |_| Err(RestoreFault::Retained)
        ),
        Err(RestoreFault::Retained)
    );
    assert_eq!(fs::read(q.join("accept-ready")).unwrap(), b"stage-1\n");
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
    let mut attempt = DirectoryQuarantine
        .begin_session(&anchored, relative)
        .unwrap();
    let mut writer = attempt.destination().unwrap();
    writer.write_all(&bytes).unwrap();
    writer.sync_all().unwrap();
    attempt.publish("conv", &digest).unwrap();
    writer.write_all(b"tampered-bytes!!").unwrap();
    writer.sync_all().unwrap();
    assert_eq!(fs::read(live.join(relative)).unwrap(), bytes);
}

#[cfg(unix)]
#[test]
fn sealed_names_are_linked_before_the_tree_is_synced() {
    let root = tempfile::tempdir().unwrap();
    let directory = open_directory(root.path(), false).unwrap();
    let records = ensure_dir_at(&directory, OsStr::new("records")).unwrap();
    let notes = ensure_dir_at(&records, OsStr::new("notes")).unwrap();
    let mut note = create_unpublished(
        &notes,
        OsStr::new("note"),
        "records/notes/note",
        false,
        RestoreFault::Tamper,
    )
    .unwrap();
    note.store(b"sealed note").unwrap();
    note.sync_file().unwrap();
    let mut journal = create_unpublished(
        &directory,
        OsStr::new("journal.txt"),
        "journal.txt",
        false,
        RestoreFault::Tamper,
    )
    .unwrap();
    journal.store(b"sealed journal").unwrap();
    journal.sync_file().unwrap();
    assert!(!root.path().join("records/notes/note").exists());
    let mut synced = false;
    publish_held(&directory, &mut [note], &mut journal, |tree| {
        assert_eq!(
            fs::read(root.path().join("records/notes/note")).unwrap(),
            b"sealed note"
        );
        assert_eq!(
            fs::read(root.path().join("journal.txt")).unwrap(),
            b"sealed journal"
        );
        sync_tree_at(tree)?;
        synced = true;
        Ok(())
    })
    .unwrap();
    assert!(synced);
}

#[cfg(unix)]
#[test]
fn a_linked_tree_sync_failure_refuses_publication() {
    let root = tempfile::tempdir().unwrap();
    let directory = open_directory(root.path(), false).unwrap();
    let mut journal = create_unpublished(
        &directory,
        OsStr::new("journal.txt"),
        "journal.txt",
        false,
        RestoreFault::Tamper,
    )
    .unwrap();
    journal.store(b"sealed journal").unwrap();
    journal.sync_file().unwrap();
    let error = publish_held(&directory, &mut [], &mut journal, |_| {
        assert_eq!(
            fs::read(root.path().join("journal.txt")).unwrap(),
            b"sealed journal"
        );
        Err(RestoreFault::Unreadable)
    })
    .unwrap_err();
    assert_eq!(error, RestoreFault::Unreadable);
}

#[cfg(unix)]
#[test]
fn linked_files_and_their_directory_refuse_writes_until_live() {
    let root = tempfile::tempdir().unwrap();
    let sealed = root.path().join("sealed");
    mkdir_exclusive(&sealed).unwrap();
    let directory = open_directory(&sealed, false).unwrap();
    // This is the attacker's directory descriptor, acquired before sealing.
    let previous_directory = open_directory(&sealed, false).unwrap();
    let mut file = create_unpublished(
        &directory,
        OsStr::new("note"),
        "note",
        false,
        RestoreFault::Tamper,
    )
    .unwrap();
    file.store(b"sealed").unwrap();
    file.sync_file().unwrap();
    file.publish_in(RestoreFault::Tamper).unwrap();
    for (read, truncate) in [(false, false), (true, false), (false, true)] {
        assert!(fs::OpenOptions::new()
            .read(read)
            .write(true)
            .truncate(truncate)
            .open(sealed.join("note"))
            .is_err());
    }
    let mut guard = ProtectedDirectories::seal(&directory).unwrap();
    assert!(file.name_is_owned().unwrap());
    assert!(unlink_at(&previous_directory, OsStr::new("note")).is_err());
    assert!(open_file_at(&previous_directory, OsStr::new("new"), OpenMode::CreateNew).is_err());
    assert!(fs::rename(sealed.join("note"), sealed.join("aside")).is_err());
    let live = root.path().join("live");
    rename_new(&sealed, &live, RestoreFault::LiveOccupied).unwrap();
    assert_eq!(fs::read(live.join("note")).unwrap(), b"sealed");
    file.release().unwrap();
    assert!(guard.release());
    fs::OpenOptions::new()
        .write(true)
        .open(live.join("note"))
        .unwrap()
        .write_all(b"usable")
        .unwrap();
    assert_eq!(fs::read(live.join("note")).unwrap(), b"usable");
}

#[cfg(unix)]
#[test]
fn an_extra_name_before_namespace_sealing_is_not_promoted() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    mkdir_exclusive(&source).unwrap();
    fs::write(source.join("journal.txt"), b"journal").unwrap();
    fs::write(source.join("note"), b"sealed").unwrap();
    let loaded = DirectoryQuarantine
        .load(&DirectoryQuarantine.anchor(&source).unwrap())
        .unwrap();
    let dest = root.path().join("dest");
    mkdir_exclusive(&dest).unwrap();
    let directory = open_directory(&dest, false).unwrap();
    let mut note = create_unpublished(
        &directory,
        OsStr::new("note"),
        "note",
        false,
        RestoreFault::Tamper,
    )
    .unwrap();
    note.store(b"sealed").unwrap();
    let mut journal = create_unpublished(
        &directory,
        OsStr::new("journal.txt"),
        "journal.txt",
        false,
        RestoreFault::Tamper,
    )
    .unwrap();
    journal.store(b"journal").unwrap();
    let mut held = vec![note];
    publish_held(&directory, &mut held, &mut journal, sync_tree_at).unwrap();
    fs::write(dest.join("unsealed-extra"), b"not in the cut").unwrap();
    let mut namespace = ProtectedDirectories::seal(&directory).unwrap();
    assert_eq!(
        published_tree_matches(&directory, &loaded, &held, &journal),
        Err(RestoreFault::Tamper)
    );
    assert!(namespace.release());
    fs::remove_file(dest.join("unsealed-extra")).unwrap();
    let mut namespace = ProtectedDirectories::seal(&directory).unwrap();
    published_tree_matches(&directory, &loaded, &held, &journal).unwrap();
    assert!(namespace.release());
}

#[cfg(unix)]
#[test]
fn a_pre_seal_leaf_substitution_is_refused_by_inode_binding() {
    let root = tempfile::tempdir().unwrap();
    let directory = open_directory(root.path(), false).unwrap();
    let mut file = create_unpublished(
        &directory,
        OsStr::new("note"),
        "note",
        false,
        RestoreFault::Tamper,
    )
    .unwrap();
    file.store(b"sealed").unwrap();
    file.publish_in(RestoreFault::Tamper).unwrap();
    fs::rename(root.path().join("note"), root.path().join("aside")).unwrap();
    fs::write(root.path().join("note"), b"substitute").unwrap();
    let mut guard = ProtectedDirectories::seal(&directory).unwrap();
    assert!(!file.name_is_owned().unwrap());
    assert!(guard.release());
    assert_eq!(fs::read(root.path().join("note")).unwrap(), b"substitute");
}

#[test]
fn dropping_a_named_reservation_does_not_unlink_its_replacement() {
    let root = tempfile::tempdir().unwrap();
    let name = root.path().join("reserved");
    fs::write(&name, b"owned").unwrap();
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&name)
        .unwrap();
    let reservation = Unpublished {
        parent: open_directory(root.path(), false).unwrap(),
        name: OsString::from("reserved"),
        relative: "reserved".into(),
        file: Some(file),
        fingerprint: Fingerprint::of(b"owned"),
        named: true,
        published: false,
        protected: false,
    };
    fs::rename(&name, root.path().join("owned-aside")).unwrap();
    fs::write(&name, b"other occupant").unwrap();
    drop(reservation);
    assert_eq!(fs::read(name).unwrap(), b"other occupant");
}

#[cfg(unix)]
#[test]
fn an_existing_publication_prevents_a_new_session_reservation() {
    let root = tempfile::tempdir().unwrap();
    let live = root.path().join("live");
    mkdir_exclusive(&live).unwrap();
    fs::create_dir(live.join("sessions")).unwrap();
    let sidecar = live.join("sessions/conv.sqlite3.publication");
    fs::write(&sidecar, b"pub-previous\n").unwrap();
    let anchored = DirectoryQuarantine.anchor(&live).unwrap();
    assert!(matches!(
        DirectoryQuarantine.begin_session(&anchored, "sessions/conv.sqlite3"),
        Err(RestoreFault::Retained)
    ));
    assert!(!live.join("sessions/conv.sqlite3").exists());
    assert!(!live.join("sessions/conv.sqlite3.partial").exists());
    assert_eq!(fs::read(sidecar).unwrap(), b"pub-previous\n");
}

#[cfg(unix)]
#[test]
fn a_session_attempt_keeps_namespace_protected_until_release() {
    let root = tempfile::tempdir().unwrap();
    let live = root.path().join("live");
    mkdir_exclusive(&live).unwrap();
    let anchored = DirectoryQuarantine.anchor(&live).unwrap();
    let (bytes, digest) = one_conversation("conv", b"sealed");
    let mut attempt = DirectoryQuarantine
        .begin_session(&anchored, "sessions/conv.sqlite3")
        .unwrap();
    attempt.destination().unwrap().write_all(&bytes).unwrap();
    attempt.publish("conv", &digest).unwrap();
    attempt.note_publication("pub-test").unwrap();
    let path = live.join("sessions/conv.sqlite3");
    assert!(fs::rename(&path, live.join("sessions/swapped")).is_err());
    assert!(fs::OpenOptions::new().write(true).open(&path).is_err());
    attempt.release().unwrap();
    fs::OpenOptions::new().write(true).open(&path).unwrap();
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[cfg(unix)]
#[test]
fn attempt_retirement_empties_only_the_owned_inode_after_a_name_swap() {
    let root = tempfile::tempdir().unwrap();
    let live = root.path().join("live");
    mkdir_exclusive(&live).unwrap();
    let anchored = DirectoryQuarantine.anchor(&live).unwrap();
    let (bytes, digest) = one_conversation("conv", b"sealed");
    let mut attempt = DirectoryQuarantine
        .begin_session(&anchored, "sessions/conv.sqlite3")
        .unwrap();
    attempt.destination().unwrap().write_all(&bytes).unwrap();
    attempt.publish("conv", &digest).unwrap();
    let path = live.join("sessions/conv.sqlite3");
    let aside = live.join("sessions/owned-aside");
    fs::rename(&path, &aside).unwrap();
    fs::write(&path, b"other occupant").unwrap();
    assert_eq!(attempt.retire(), Err(RestoreFault::Retained));
    assert!(fs::read(aside).unwrap().is_empty());
    assert_eq!(fs::read(path).unwrap(), b"other occupant");
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
    assert_eq!(error.raw_os_error(), Some(5));
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
    let mut attempt = DirectoryQuarantine
        .begin_session(&anchored, relative)
        .unwrap();
    attempt.destination().unwrap().write_all(&bytes).unwrap();
    let error = attempt.publish("conv", &digest).unwrap_err();
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
fn a_failed_source_retirement_preserves_the_live_copy_and_syncs() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("quarantine");
    let live = root.path().join("live");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("accept-ready"), b"stage-1\n").unwrap();
    fs::create_dir(&live).unwrap();
    fs::write(live.join("note"), b"sealed bytes").unwrap();
    let parent = open_directory(root.path(), false).unwrap();
    let mut synced = false;
    let promoted = finish_promotion(
        || {
            assert_eq!(fs::read(live.join("note")).unwrap(), b"sealed bytes");
            Err(RestoreFault::Unreadable)
        },
        || {
            sync_dir(&parent).unwrap();
            synced = true;
            true
        },
    );
    assert!(promoted.source_retirement_failed);
    assert!(promoted.durable);
    assert!(synced);
    assert_eq!(fs::read(live.join("note")).unwrap(), b"sealed bytes");
    assert_eq!(fs::read(source.join("accept-ready")).unwrap(), b"stage-1\n");
}

#[test]
fn source_retirement_and_parent_sync_failures_are_kept_together() {
    let mut sync_attempted = false;
    let promoted = finish_promotion(
        || Err(RestoreFault::Retained),
        || {
            sync_attempted = true;
            false
        },
    );
    assert!(promoted.source_retirement_failed);
    assert!(!promoted.durable);
    assert!(sync_attempted);
    let promoted = finish_promotion(|| Ok(()), || true);
    assert!(!promoted.source_retirement_failed);
    assert!(promoted.durable);
}

#[test]
fn a_renamed_source_is_retired_through_its_handle() {
    let root = tempfile::tempdir().unwrap();
    let q = root.path().join("q");
    fs::create_dir(&q).unwrap();
    fs::write(q.join("accept-ready"), b"stage-1\n").unwrap();
    let directory = open_directory(&q, false).unwrap();
    let marker = open_file_at(&directory, OsStr::new("accept-ready"), OpenMode::Read).unwrap();
    let id = tree_id_of(&directory).unwrap();
    let saved = root.path().join("saved");
    fs::rename(&q, &saved).unwrap();
    retire_source_marker(&directory, &id, &marker).unwrap();
    assert!(saved.is_dir());
    assert!(fs::read(saved.join("accept-ready")).unwrap().is_empty());
    assert!(!q.exists());
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

#[cfg(target_os = "macos")]
#[test]
fn mac_session_source_scratch_stays_outside_the_sealed_tree() {
    let root = tempfile::tempdir().unwrap();
    let live = root.path().join("live");
    mkdir_exclusive(&live).unwrap();
    let anchored = DirectoryQuarantine.anchor(&live).unwrap();
    let (bytes, digest) = one_conversation("conv", b"sealed");
    let mut attempt = DirectoryQuarantine
        .begin_session(&anchored, "sessions/conv.sqlite3")
        .unwrap();
    let mut source = attempt.destination().unwrap();
    let scratch = path_of_handle(&source).unwrap();
    assert_eq!(
        scratch.parent(),
        Some(live.parent().unwrap().canonicalize().unwrap().as_path())
    );
    source.write_all(&bytes).unwrap();
    attempt.publish("conv", &digest).unwrap();
    assert_eq!(source.metadata().unwrap().len(), 0);
    assert!(!live.join("sessions/conv.sqlite3.partial").exists());
    source.seek(SeekFrom::Start(0)).unwrap();
    source.write_all(b"later source writer").unwrap();
    let output = live.join("sessions/conv.sqlite3");
    assert_eq!(fs::read(&output).unwrap(), bytes);
    assert!(fs::OpenOptions::new().write(true).open(&output).is_err());
    assert_eq!(attempt.retire(), Err(RestoreFault::Retained));
    assert_eq!(source.metadata().unwrap().len(), 0);
}

#[cfg(target_os = "macos")]
#[test]
fn mac_named_output_refuses_other_writers_from_creation() {
    let root = tempfile::tempdir().unwrap();
    let parent = open_directory(root.path(), false).unwrap();
    let mut output = create_unpublished(
        &parent,
        OsStr::new("note"),
        "note",
        true,
        RestoreFault::Retained,
    )
    .unwrap();
    let identity = tree_id_of(output.file.as_ref().unwrap()).unwrap();
    let path = root.path().join("note");
    assert!(fs::OpenOptions::new().write(true).open(&path).is_err());
    output.store(b"private writer").unwrap();
    output.publish_in(RestoreFault::Retained).unwrap();
    assert_eq!(tree_id(&path).unwrap(), identity);
    assert_eq!(fs::read(&path).unwrap(), b"private writer");
    assert!(fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&path)
        .is_err());
    output.release().unwrap();
    fs::OpenOptions::new().write(true).open(&path).unwrap();
}

#[cfg(unix)]
#[test]
fn namespace_sealing_never_adopts_a_substituted_directory() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    mkdir_exclusive(&source).unwrap();
    mkdir_exclusive(&source.join("nested")).unwrap();
    fs::write(source.join("nested/note"), b"sealed").unwrap();
    let dest = root.path().join("dest");
    mkdir_exclusive(&dest).unwrap();
    let source_handle = open_directory(&source, false).unwrap();
    let destination = open_directory(&dest, false).unwrap();
    let mut held = Vec::new();
    let mut directories = Vec::new();
    copy_at(&source_handle, &destination, &mut held, &mut directories).unwrap();
    assert_eq!(directories.len(), 1);
    let original = dest.join("nested");
    let aside = root.path().join("owned-aside");
    fs::rename(&original, &aside).unwrap();
    let foreign = root.path().join("foreign");
    mkdir_exclusive(&foreign).unwrap();
    fs::write(foreign.join("note"), b"sealed").unwrap();
    fs::set_permissions(&foreign, fs::Permissions::from_mode(0o750)).unwrap();
    fs::rename(&foreign, &original).unwrap();
    assert!(matches!(
        ProtectedDirectories::seal_owned(&destination, &directories),
        Err(RestoreFault::Tamper)
    ));
    assert_eq!(
        fs::metadata(&original).unwrap().permissions().mode() & 0o777,
        0o750
    );
    assert_eq!(fs::read(original.join("note")).unwrap(), b"sealed");
    assert_eq!(
        fs::metadata(&aside).unwrap().permissions().mode() & 0o777,
        0o700
    );
    // Positive control: the same retained directory is accepted once its
    // expected name is restored; an unrelated inserted subtree stays untouched.
    fs::rename(&original, &foreign).unwrap();
    fs::rename(&aside, &original).unwrap();
    let unexpected = dest.join("unexpected");
    mkdir_exclusive(&unexpected).unwrap();
    fs::set_permissions(&unexpected, fs::Permissions::from_mode(0o550)).unwrap();
    let mut guard = ProtectedDirectories::seal_owned(&destination, &directories).unwrap();
    assert_eq!(
        fs::metadata(&original).unwrap().permissions().mode() & 0o777,
        0o500
    );
    assert_eq!(
        fs::metadata(&unexpected).unwrap().permissions().mode() & 0o777,
        0o550
    );
    assert!(guard.release());
    assert_eq!(
        fs::metadata(&original).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&unexpected).unwrap().permissions().mode() & 0o777,
        0o550
    );
}

#[cfg(unix)]
#[test]
fn a_single_directory_guard_preserves_modes_and_ignores_children() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let sessions = root.path().join("sessions");
    mkdir_exclusive(&sessions).unwrap();
    let foreign = sessions.join("foreign");
    mkdir_exclusive(&foreign).unwrap();
    fs::set_permissions(&foreign, fs::Permissions::from_mode(0o550)).unwrap();
    fs::set_permissions(&sessions, fs::Permissions::from_mode(0o750)).unwrap();
    let directory = open_directory(&sessions, false).unwrap();
    let mut guard = ProtectedDirectories::seal(&directory).unwrap();
    assert_eq!(
        fs::metadata(&foreign).unwrap().permissions().mode() & 0o777,
        0o550
    );
    assert!(guard.release());
    assert_eq!(
        fs::metadata(&sessions).unwrap().permissions().mode() & 0o777,
        0o750
    );
    assert_eq!(
        fs::metadata(&foreign).unwrap().permissions().mode() & 0o777,
        0o550
    );
}

#[cfg(unix)]
#[test]
fn all_retained_directory_guards_restore_their_captured_modes() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let child = root.path().join("owned-child");
    mkdir_exclusive(&child).unwrap();
    let parent = open_directory(root.path(), false).unwrap();
    let directory = open_directory(&child, false).unwrap();
    let owned = vec![OwnedDirectory {
        relative: "owned-child".into(),
        parent: duplicate(&parent).unwrap(),
        name: OsString::from("owned-child"),
        directory,
    }];
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o750)).unwrap();
    fs::set_permissions(&child, fs::Permissions::from_mode(0o550)).unwrap();
    let mut guard = ProtectedDirectories::seal_owned(&parent, &owned).unwrap();
    assert_eq!(
        fs::metadata(&child).unwrap().permissions().mode() & 0o777,
        0o500
    );
    assert!(guard.release());
    assert_eq!(
        fs::metadata(root.path()).unwrap().permissions().mode() & 0o777,
        0o750
    );
    assert_eq!(
        fs::metadata(&child).unwrap().permissions().mode() & 0o777,
        0o550
    );
}

#[cfg(unix)]
#[test]
fn a_session_parent_symlink_is_refused_before_writing_outside() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let live = root.path().join("live");
    mkdir_exclusive(&live).unwrap();
    let outside = root.path().join("outside");
    mkdir_exclusive(&outside).unwrap();
    fs::write(outside.join("sentinel"), b"untouched").unwrap();
    symlink(&outside, live.join("sessions")).unwrap();
    let anchored = DirectoryQuarantine.anchor(&live).unwrap();
    assert!(DirectoryQuarantine
        .begin_session(&anchored, "sessions/conv.sqlite3")
        .is_err());
    assert!(fs::symlink_metadata(live.join("sessions"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"untouched");
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn captured_directories_must_be_private_owned_and_empty() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let candidate = root.path().join("candidate");
    mkdir_exclusive(&candidate).unwrap();
    let parent = open_directory(root.path(), false).unwrap();
    let name = OsStr::new("candidate");
    let directory = capture_created_directory(&parent, name).unwrap();
    assert!(captured_directory_is_empty(&directory).unwrap());
    fs::write(candidate.join("sentinel"), b"foreign").unwrap();
    assert!(matches!(
        capture_created_directory(&parent, name),
        Err(RestoreFault::Retained)
    ));
    assert_eq!(fs::read(candidate.join("sentinel")).unwrap(), b"foreign");
    assert_eq!(
        fs::metadata(&candidate).unwrap().permissions().mode() & 0o777,
        0o700
    );
    fs::remove_file(candidate.join("sentinel")).unwrap();
    fs::set_permissions(&candidate, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        capture_created_directory(&parent, name),
        Err(RestoreFault::Retained)
    ));
    assert_eq!(
        fs::metadata(&candidate).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert!(fs::read_dir(&candidate).unwrap().next().is_none());
    fs::set_permissions(&candidate, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(capture_created_directory(&parent, name).is_ok());
}

#[cfg(unix)]
#[test]
fn a_held_session_cut_survives_unlink_and_ancestor_replacement() {
    let root = tempfile::tempdir().unwrap();
    let ancestor = root.path().join("parent");
    mkdir_exclusive(&ancestor).unwrap();
    let original_path = ancestor.join("conv.sqlite3");
    let (original, original_digest) = one_conversation("conv", b"retained original");
    let (replacement, replacement_digest) = one_conversation("conv", b"different replacement");
    assert_ne!(original_digest, replacement_digest);
    fs::write(&original_path, &original).unwrap();
    let directory = open_directory(&ancestor, false).unwrap();
    let mut held = open_file_at(&directory, OsStr::new("conv.sqlite3"), OpenMode::Read).unwrap();
    fs::remove_file(&original_path).unwrap();
    fs::rename(&ancestor, root.path().join("aside")).unwrap();
    mkdir_exclusive(&ancestor).unwrap();
    fs::write(&original_path, &replacement).unwrap();
    // The pathname is a valid different session, rather than an invalid fixture.
    assert_eq!(
        DirectoryQuarantine
            .session_cut(&original_path, "conv")
            .unwrap()
            .digest,
        replacement_digest
    );
    assert_eq!(
        held_session_cut(&mut held, "conv").unwrap().digest,
        original_digest
    );
    assert_eq!(fs::read(&original_path).unwrap(), replacement);
}

#[cfg(windows)]
#[test]
fn closed_stage_tokens_reacquire_the_exact_files_after_root_rename() {
    let temp = tempfile::tempdir().unwrap();
    let parent = open_ancestor(temp.path()).unwrap();
    let root = create_directory_handle(&parent, OsStr::new("partial"), true).unwrap();
    let mut directories = Vec::new();
    let directory = owned_directory_under(&root, "nested/note", &mut directories).unwrap();
    let mut file = open_file_at(&directory, OsStr::new("note"), OpenMode::CreateNew).unwrap();
    file.write_all(b"owned bytes").unwrap();
    file.sync_all().unwrap();
    let mut files = vec![OwnedStageFile {
        relative: "nested/note".into(),
        file,
        fingerprint: Fingerprint::of(b"owned bytes"),
    }];
    drop(directory);
    let mut namespace = ProtectedDirectories::seal_owned(&root, &directories).unwrap();
    staged_tree_matches(&root, &files, &directories).unwrap();
    let closed = closed_windows::ClosedTree::capture_stage(&root, &files, &directories).unwrap();
    files.clear();
    directories.clear();
    namespace.close_for_rename();
    rename_owned_at(
        &root,
        &parent,
        OsStr::new("partial"),
        &parent,
        OsStr::new("staged"),
        RestoreFault::QuarantineOccupied,
    )
    .unwrap();
    let files = closed.reacquire_stage(&root).unwrap();
    assert_eq!(files.len(), 1);
    // Audit failure after finish must still retire the originally staged inode.
    empty_owned_file(&files[0].file).unwrap();
    assert!(closed.release(&root));
    assert!(fs::read(temp.path().join("staged/nested/note"))
        .unwrap()
        .is_empty());
}

#[cfg(windows)]
#[test]
fn closed_tree_identity_mismatch_does_not_release_or_empty_another_tree() {
    let temp = tempfile::tempdir().unwrap();
    let parent = open_ancestor(temp.path()).unwrap();
    let original = create_directory_handle(&parent, OsStr::new("original"), true).unwrap();
    let other = create_directory_handle(&parent, OsStr::new("other"), true).unwrap();
    let mut original_file =
        open_file_at(&original, OsStr::new("note"), OpenMode::CreateNew).unwrap();
    original_file.write_all(b"original").unwrap();
    let mut other_file = open_file_at(&other, OsStr::new("note"), OpenMode::CreateNew).unwrap();
    other_file.write_all(b"unrelated").unwrap();
    let files = vec![OwnedStageFile {
        relative: "note".into(),
        file: original_file,
        fingerprint: Fingerprint::of(b"original"),
    }];
    let closed = closed_windows::ClosedTree::capture_stage(&original, &files, &[]).unwrap();
    let before = nessa_local_storage::DirectoryAcl::capture(&other).unwrap();
    assert!(!closed.release(&other));
    assert!(closed.reacquire_stage(&other).is_err());
    assert_eq!(closed.retire(&other), Err(RestoreFault::Retained));
    assert_eq!(
        nessa_local_storage::DirectoryAcl::capture(&other).unwrap(),
        before
    );
    let mut bytes = Vec::new();
    other_file.seek(SeekFrom::Start(0)).unwrap();
    other_file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"unrelated");
    empty_owned_file(&files[0].file).unwrap();
    empty_owned_file(&other_file).unwrap();
    nessa_local_storage::release_file_writes(&files[0].file).unwrap();
    nessa_local_storage::release_file_writes(&other_file).unwrap();
    nessa_local_storage::release_directory_writes(&original).unwrap();
    nessa_local_storage::release_directory_writes(&other).unwrap();
}

#[cfg(windows)]
#[test]
fn windows_real_session_publication_does_not_claim_directory_durability() {
    let temp = tempfile::tempdir().unwrap();
    let live = temp.path().join("live");
    let parent = open_ancestor(temp.path()).unwrap();
    let root = create_directory_handle(&parent, OsStr::new("live"), true).unwrap();
    nessa_local_storage::release_directory_writes(&root).unwrap();
    drop(root);
    let anchored = DirectoryQuarantine.anchor(&live).unwrap();
    let (bytes, digest) = one_conversation("conv", b"publication");
    let mut attempt = DirectoryQuarantine
        .begin_session(&anchored, "sessions/conv.sqlite3")
        .unwrap();
    attempt.destination().unwrap().write_all(&bytes).unwrap();
    attempt.publish("conv", &digest).unwrap();
    assert!(!attempt.note_publication("pub-durability").unwrap().durable);
    attempt.release().unwrap();
    assert_eq!(fs::read(live.join("sessions/conv.sqlite3")).unwrap(), bytes);
}

#[cfg(windows)]
#[test]
fn windows_real_promotion_keeps_live_bytes_without_claiming_directory_durability() {
    let temp = tempfile::tempdir().unwrap();
    let quarantine = DirectoryQuarantine
        .anchor(&temp.path().join("quarantine"))
        .unwrap();
    let live = DirectoryQuarantine
        .anchor(&temp.path().join("live"))
        .unwrap();
    let mut stage = DirectoryQuarantine.begin(&quarantine, &live).unwrap();
    stage.write("note", b"publication").unwrap();
    let mut owner = stage.finish("journal").unwrap();
    owner.mark_ready("stage-durability").unwrap();
    drop(owner);
    let loaded = DirectoryQuarantine.load(&quarantine).unwrap();
    let promoted = DirectoryQuarantine
        .promote(&quarantine, &live, &loaded, "stage-durability")
        .unwrap();
    assert!(!promoted.durable);
    assert!(!promoted.publication_restricted);
    assert!(!promoted.source_retirement_failed);
    assert_eq!(fs::read(live.join("note")).unwrap(), b"publication");
    assert!(fs::read(quarantine.join("accept-ready"))
        .unwrap()
        .is_empty());
}

#[cfg(windows)]
#[test]
fn reacquired_stage_content_is_rechecked_before_permissions_are_released() {
    let temp = tempfile::tempdir().unwrap();
    let parent = open_ancestor(temp.path()).unwrap();
    let root = create_directory_handle(&parent, OsStr::new("partial"), true).unwrap();
    let mut file = open_file_at(&root, OsStr::new("note"), OpenMode::CreateNew).unwrap();
    file.write_all(b"expected").unwrap();
    file.sync_all().unwrap();
    let mut files = vec![OwnedStageFile {
        relative: "note".into(),
        file,
        fingerprint: Fingerprint::of(b"expected"),
    }];
    let mut namespace = ProtectedDirectories::seal_owned(&root, &[]).unwrap();
    staged_tree_matches(&root, &files, &[]).unwrap();
    verify_reacquired_stage(&root, &files).unwrap();
    let closed = closed_windows::ClosedTree::capture_stage(&root, &files, &[]).unwrap();
    // Model changed bytes between capture and exclusive reacquisition. Native
    // storage separately proves an ordinary writer can write and close during
    // its temporary FRFW permission window before trusted reacquisition wins.
    files[0].file.seek(SeekFrom::Start(0)).unwrap();
    files[0].file.write_all(b"changed!").unwrap();
    files[0].file.sync_all().unwrap();
    files.clear();
    namespace.close_for_rename();
    rename_owned_at(
        &root,
        &parent,
        OsStr::new("partial"),
        &parent,
        OsStr::new("staged"),
        RestoreFault::QuarantineOccupied,
    )
    .unwrap();
    let files = closed.reacquire_stage(&root).unwrap();
    assert_eq!(
        verify_reacquired_stage(&root, &files),
        Err(RestoreFault::Retained)
    );
    assert!(fs::read(temp.path().join("staged/note"))
        .unwrap()
        .is_empty());
    // Explicit fixture cleanup happens only after the retained/zero assertions.
    assert!(closed.release(&root));
}
