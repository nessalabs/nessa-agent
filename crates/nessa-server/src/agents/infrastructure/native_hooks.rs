//! The native-hooks choice file and its audit records.
//!
//! ```text
//! set -> requested record -> pending marker -> native-hooks.json -> outcome record
//!     -> atomic flag -> marker removed
//! open -> pending marker settled or put back -> native-hooks.json -> flags
//!      (unreadable or invalid documents load as off)
//! process start -> switch.allowed() once -> that start's profile and environment
//! ```
//!
//! Arrows are the order of one change, of one startup read, and of one
//! process start. The flag moves last, so a process start never runs under
//! a choice the audit has not recorded. A failed requested record writes
//! nothing. A failed outcome record reverts the file, unless the outcome
//! record can be read back: then the change stands and the flag follows,
//! because the record says it took effect. A file that was renamed into
//! place and then failed its directory sync is the new choice: its outcome
//! is recorded and the flag follows, and the command still reports the
//! storage failure. A retry of the same
//! principal, harness, and request id keeps the record already written,
//! including its observation time, when the choice matches, and fails when
//! that id was used for a different choice. The adapter assigns the record id
//! and the observation time. There is no provider process in this transition:
//! each process start reads the flag once, including the next start of a
//! conversation already open. A process already running is not changed.
//!
//! A crash after the file is published and before the outcome record leaves
//! the pending marker behind. The marker holds the whole outcome record it
//! waits for. The next start, or the next command, settles it first: an
//! outcome record that agrees with it, once the audit directory syncs, has the
//! file written again to the choice it records. Anything else puts the file
//! back to the choice before the command, and back on only when that
//! command's requested record says hooks were on. So the file never runs
//! ahead of the audit across a restart. A retry of that command finishes it.

use crate::agents::application::{
    selection_from_stored, NativeHooksAgent, NativeHooksAudit, NativeHooksChange, NativeHooksError,
    NativeHooksEvidence, NativeHooksPhase, NativeHooksSelection,
};
use nessa_auth::application::ports::Clock;
use nessa_local_storage::{create_directory, open, sync_directory, OpenMode, PrivateTempFile};
use nessa_sdk::application::agent_execution::providers::NativeHooksSwitch;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

/// The choice file, the flags launches read, and the audit port.
pub struct NativeHooksPreferences {
    directory: PathBuf,
    claude: Arc<AtomicBool>,
    codex: Arc<AtomicBool>,
    gate: Arc<Mutex<()>>,
    audit: Arc<dyn NativeHooksAudit>,
    sync: DirectorySync,
}

/// How the choice file's directory is made durable after a rename.
type DirectorySync = fn(&Path) -> std::io::Result<()>;

impl NativeHooksPreferences {
    /// Load `native-hooks.json` under `directory`, or off when it is missing
    /// or invalid. Creates the audit directory.
    pub fn open(directory: PathBuf, clock: Arc<dyn Clock>) -> Result<Arc<Self>, NativeHooksError> {
        let audit = Arc::new(DurableNativeHooksAudit::new(
            directory.join("audit").join("native-hooks"),
            clock,
        )?);
        Self::open_with(directory, audit)
    }

    /// [`open`](Self::open) with an audit port the caller supplies.
    pub fn open_with(
        directory: PathBuf,
        audit: Arc<dyn NativeHooksAudit>,
    ) -> Result<Arc<Self>, NativeHooksError> {
        Self::open_syncing(directory, audit, sync_directory)
    }

    fn open_syncing(
        directory: PathBuf,
        audit: Arc<dyn NativeHooksAudit>,
        sync: DirectorySync,
    ) -> Result<Arc<Self>, NativeHooksError> {
        create_directory(&directory).map_err(|_| NativeHooksError::Storage)?;
        let stored = match settle_pending(&directory, sync) {
            Ok(Some(settled)) => settled,
            Ok(None) => read_choice(&directory.join(CHOICE)),
            // This start uses the choice the audit last recorded, and the
            // marker stays for the next start or command.
            Err((selection, error)) => {
                tracing::warn!(?error, "native hooks pending change could not be settled");
                selection
            }
        };
        Ok(Arc::new(Self {
            directory,
            claude: Arc::new(AtomicBool::new(stored.claude())),
            codex: Arc::new(AtomicBool::new(stored.codex())),
            gate: Arc::new(Mutex::new(())),
            audit,
            sync,
        }))
    }

    /// The choice in effect now.
    pub fn selection(&self) -> NativeHooksSelection {
        NativeHooksSelection::new(
            self.claude.load(Ordering::Relaxed),
            self.codex.load(Ordering::Relaxed),
        )
    }

    /// The flag a Claude or Codex process start reads. OpenCode is not
    /// represented. Each start reads [`NativeHooksSwitch::allowed`] once.
    pub fn switch(&self, agent: NativeHooksAgent) -> NativeHooksSwitch {
        let flag = match agent {
            NativeHooksAgent::Claude => self.claude.clone(),
            NativeHooksAgent::Codex => self.codex.clone(),
        };
        NativeHooksSwitch::from_shared(flag)
    }

    /// Record the request, write the file, then record the outcome.
    ///
    /// A retry of the same request id keeps the records already written when
    /// they are for this choice. Failure of either record, or of the file,
    /// returns an error. The flag moves only after the outcome record: a file
    /// renamed into place counts as written even when its directory sync
    /// failed. An outcome-record failure puts the file back, unless the
    /// outcome record can be read back; a file that cannot be put back is left
    /// to the pending marker, and the flag keeps the recorded choice.
    pub async fn set(
        &self,
        change: NativeHooksChange,
    ) -> Result<NativeHooksSelection, NativeHooksError> {
        let directory = self.directory.clone();
        let claude = self.claude.clone();
        let codex = self.codex.clone();
        let audit = self.audit.clone();
        let gate = self.gate.clone();
        let sync = self.sync;
        tokio::task::spawn_blocking(move || {
            let _guard = gate.lock().unwrap_or_else(|error| error.into_inner());
            set_blocking(directory, claude, codex, audit, sync, change)
        })
        .await
        // A panicked or cancelled task did not finish. That is not an audit
        // failure, and it is not the file write's own error.
        .map_err(|_| NativeHooksError::Interrupted)?
    }
}

fn set_blocking(
    directory: PathBuf,
    claude: Arc<AtomicBool>,
    codex: Arc<AtomicBool>,
    audit: Arc<dyn NativeHooksAudit>,
    sync: DirectorySync,
    change: NativeHooksChange,
) -> Result<NativeHooksSelection, NativeHooksError> {
    // A change an earlier command left pending is settled first, so a new
    // marker never replaces one that still has work to do.
    // The flags follow the settled choice even when the marker stays: a put
    // back file may be in place without its directory sync.
    let settled = settle_pending(&directory, sync);
    if let Ok(Some(selection)) | Err((selection, _)) = &settled {
        store(
            &claude,
            &codex,
            NativeHooksAgent::Claude,
            selection.claude(),
        );
        store(&claude, &codex, NativeHooksAgent::Codex, selection.codex());
    }
    if let Err((_, error)) = settled {
        return Err(error);
    }
    // A retry of a command that already recorded its outcome keeps that
    // record. The before value in it is the one from the first attempt.
    // It succeeds only once the outcome record's directory and the choice's
    // directory are synced: the first attempt may have found either sync
    // failing after the record or the file was already readable.
    if already_recorded(&directory, &change)? {
        sync(&directory).map_err(|_| NativeHooksError::Storage)?;
        sync(&audit_directory(&directory)).map_err(|_| NativeHooksError::Audit)?;
        return Ok(selection_of(&claude, &codex));
    }
    let current = flag(&claude, &codex, change.agent);
    let before = recorded_before(&directory, &change)?.unwrap_or(current);
    audit.record(&change.evidence(NativeHooksPhase::Requested, before, None))?;
    if current == change.enabled {
        audit.record(&change.evidence(NativeHooksPhase::Outcome, before, Some(current)))?;
        return Ok(selection_of(&claude, &codex));
    }
    let updated = selection_of(&claude, &codex).with(change.agent, change.enabled);
    // Name the change before the file moves, so a gateway that stops before
    // the outcome is recorded puts the file back at its next start.
    let pending = PendingChoice {
        // The marker holds the whole outcome record it waits for. The
        // observation time is assigned when the record is written, and is
        // not compared.
        outcome: StoredRecord::new(
            &change.evidence(NativeHooksPhase::Outcome, before, Some(change.enabled)),
            0,
        ),
        before: current,
    };
    if publish(&directory, PENDING, &pending, sync).is_err() {
        // A marker that did land only names a change that is not in the file,
        // and the next start's revert of it changes nothing.
        return Err(NativeHooksError::Storage);
    }
    // Process starts keep reading the earlier choice until the outcome record
    // is durable: the flag moves last, so no start runs under a choice the
    // audit has not recorded.
    let written = match write_choice(&directory, &updated, sync) {
        Ok(()) => Ok(()),
        // Readers already see this file, so it is the choice to record. Its
        // durability is not confirmed, and the command says so.
        Err(Publication::Unconfirmed) => Err(NativeHooksError::Storage),
        Err(Publication::Unpublished) => {
            clear_pending(&directory, sync);
            return Err(NativeHooksError::Storage);
        }
    };
    if let Err(error) =
        audit.record(&change.evidence(NativeHooksPhase::Outcome, before, Some(change.enabled)))
    {
        // An outcome record that reads back says the change took effect. A
        // rollback now would contradict it, and a retry would stop at it.
        // The marker stays: the record's directory may not be synced, and
        // the next command or start syncs it before clearing the marker.
        match already_recorded(&directory, &change) {
            Ok(true) => {
                store(&claude, &codex, change.agent, change.enabled);
                return Err(error);
            }
            Ok(false) => {}
            // A record that cannot be read back may still be this outcome,
            // so neither the file nor its revert is known to match the audit.
            // The marker stays, and until it settles that harness is off.
            Err(_) => {
                store(&claude, &codex, change.agent, false);
                return Err(error);
            }
        }
        let reverted = updated.with(change.agent, current);
        // The flag stays at the choice the audit recorded. When the file could
        // not be put back, or its directory did not sync, the marker stays and
        // the next command or start puts it back.
        if write_choice(&directory, &reverted, sync) == Ok(()) {
            clear_pending(&directory, sync);
        }
        return Err(error);
    }
    store(&claude, &codex, change.agent, change.enabled);
    // A file whose directory did not sync could still lose its rename in a
    // crash, so the marker stays until a later settlement writes the
    // recorded choice again and syncs it.
    if written.is_ok() {
        clear_pending(&directory, sync);
    }
    written.map(|()| selection_of(&claude, &codex))
}

/// How far a failed publication got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Publication {
    /// The destination still holds what it held before.
    Unpublished,
    /// The destination holds the new document, but its directory was not
    /// synced. A reader sees the new document.
    Unconfirmed,
}

fn flag(claude: &AtomicBool, codex: &AtomicBool, agent: NativeHooksAgent) -> bool {
    match agent {
        NativeHooksAgent::Claude => claude.load(Ordering::Relaxed),
        NativeHooksAgent::Codex => codex.load(Ordering::Relaxed),
    }
}

fn store(claude: &AtomicBool, codex: &AtomicBool, agent: NativeHooksAgent, enabled: bool) {
    match agent {
        NativeHooksAgent::Claude => claude.store(enabled, Ordering::Relaxed),
        NativeHooksAgent::Codex => codex.store(enabled, Ordering::Relaxed),
    };
}

fn selection_of(claude: &AtomicBool, codex: &AtomicBool) -> NativeHooksSelection {
    NativeHooksSelection::new(
        claude.load(Ordering::Relaxed),
        codex.load(Ordering::Relaxed),
    )
}

/// The written choice is about 30 bytes. Reading stops past this, so a
/// damaged or replaced file cannot make the gateway hold it all in memory.
const MAX_CHOICE_BYTES: u64 = 4096;

/// A choice file that cannot be opened, read, or is larger than
/// `MAX_CHOICE_BYTES` is off. A missing file is the
/// first start and stays quiet. Every other failure is logged: off is the
/// safe direction, and it must not stop the gateway.
fn read_choice(path: &Path) -> NativeHooksSelection {
    let file = match open(path, OpenMode::Read) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return NativeHooksSelection::off(),
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "native hooks choice file could not be opened; loading as off");
            return NativeHooksSelection::off();
        }
    };
    let mut bytes = Vec::new();
    if let Err(error) = file.take(MAX_CHOICE_BYTES + 1).read_to_end(&mut bytes) {
        tracing::warn!(%error, path = %path.display(), "native hooks choice file could not be read; loading as off");
        return NativeHooksSelection::off();
    }
    if bytes.len() as u64 > MAX_CHOICE_BYTES {
        tracing::warn!(path = %path.display(), "native hooks choice file is larger than any choice; loading as off");
        return NativeHooksSelection::off();
    }
    selection_from_stored(&bytes)
}

fn write_choice(
    directory: &Path,
    selection: &NativeHooksSelection,
    sync: DirectorySync,
) -> Result<(), Publication> {
    publish(
        directory,
        CHOICE,
        &json!({"claude": selection.claude(), "codex": selection.codex()}),
        sync,
    )
}

/// Replace `name` under `directory` with `value`, through a private temporary
/// file renamed into place.
fn publish(
    directory: &Path,
    name: &str,
    value: &impl Serialize,
    sync: DirectorySync,
) -> Result<(), Publication> {
    let destination = directory.join(name);
    let mut file = PrivateTempFile::new_in(directory).map_err(|_| Publication::Unpublished)?;
    serde_json::to_writer(file.as_file_mut(), value).map_err(|_| Publication::Unpublished)?;
    file.as_file_mut()
        .write_all(b"\n")
        .and_then(|_| file.as_file().sync_all())
        .map_err(|_| Publication::Unpublished)?;
    file.persist(&destination)
        .map_err(|_| Publication::Unpublished)?;
    sync(directory).map_err(|_| Publication::Unconfirmed)
}

/// The choice file.
const CHOICE: &str = "native-hooks.json";

/// The change a command is about to publish, written before the choice file
/// moves and removed once the outcome is recorded and its directory synced,
/// or the file is put back.
const PENDING: &str = "native-hooks.pending.json";

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingChoice {
    /// The outcome record that settles this change, field for field apart
    /// from its observation time.
    outcome: StoredRecord,
    /// The choice to put back when the outcome was never recorded.
    before: bool,
}

/// Settle a change an earlier command left pending. `Ok(None)` means there
/// was none. `Ok(Some)` is the choice in the file once the marker is gone.
/// `Err` holds the choice to use while the marker stays, and why it stays.
///
/// An outcome record that agrees with the marker settles the change, once its
/// directory is synced and the file is written to the choice it records. Any
/// other record, or none, puts the file back to the choice before the change,
/// so the file never runs ahead of the audit. Hooks go back on only when the
/// command's requested record says they were on. A
/// marker that cannot be read is treated like a broken choice file:
/// everything is off, and the file is written off so that the marker can go
/// and later commands are not refused.
#[allow(clippy::type_complexity)]
fn settle_pending(
    directory: &Path,
    sync: DirectorySync,
) -> Result<Option<NativeHooksSelection>, (NativeHooksSelection, NativeHooksError)> {
    let pending = match read_pending(&directory.join(PENDING)) {
        Ok(None) => return Ok(None),
        Ok(Some(pending)) => pending,
        Err(()) => {
            tracing::warn!("native hooks pending change could not be read; loading as off");
            return put_back(directory, NativeHooksSelection::off(), sync);
        }
    };
    let stored = read_choice(&directory.join(CHOICE));
    let agent = match pending.outcome.target.agent.as_str() {
        "claude" => NativeHooksAgent::Claude,
        _ => NativeHooksAgent::Codex,
    };
    let outcome = audit_record(directory, &pending.outcome.record_id);
    let agreed = match read_bounded(&outcome) {
        // A record that cannot be read may still be the outcome, so neither
        // keeping nor putting back the file is known to match the audit. The
        // marker stays, and until it settles that harness is off.
        Err(()) => return Err((stored.with(agent, false), NativeHooksError::Audit)),
        Ok(None) => false,
        // Records are renamed into place whole, so one that does not parse,
        // or is larger than any record, is not this change's outcome.
        Ok(Some(bytes)) => serde_json::from_slice::<StoredRecord>(&bytes).is_ok_and(|stored| {
            StoredRecord {
                observed_at_ms: pending.outcome.observed_at_ms,
                ..stored
            } == pending.outcome
        }),
    };
    if agreed {
        // The command that wrote the record may have stopped before its
        // directory synced.
        if sync(&audit_directory(directory)).is_err() {
            return Err((stored, NativeHooksError::Audit));
        }
        // The file's own rename may not have survived either. The record
        // decides: the file is written to say what it says, and synced.
        let recorded = pending.outcome.transition.after.unwrap_or(pending.before);
        return put_back(directory, stored.with(agent, recorded), sync);
    }
    // Putting hooks back on needs the audit to say they were on: the same
    // command's requested record must agree with the marker. Off needs no
    // proof.
    let before = pending.before && requested_agrees(directory, agent, &pending);
    put_back(directory, stored.with(agent, before), sync)
}

/// True when the requested record of the marker's command is stored and says
/// the choice before it was `pending.before`.
fn requested_agrees(directory: &Path, agent: NativeHooksAgent, pending: &PendingChoice) -> bool {
    let outcome = &pending.outcome;
    let expected = StoredRecord {
        record_id: record_name(
            agent,
            &outcome.initiator.principal_id,
            &outcome.correlation_id,
            NativeHooksPhase::Requested,
        ),
        kind: outcome.kind.clone(),
        target: StoredTarget {
            agent: outcome.target.agent.clone(),
        },
        transition: StoredTransition {
            before: pending.before,
            requested: outcome.transition.requested,
            after: None,
            phase: NativeHooksPhase::Requested.name().into(),
        },
        cause: outcome.cause.clone(),
        initiator: StoredInitiator {
            kind: outcome.initiator.kind.clone(),
            principal_id: outcome.initiator.principal_id.clone(),
            surface_id: outcome.initiator.surface_id.clone(),
            organization_id: outcome.initiator.organization_id.clone(),
        },
        correlation_id: outcome.correlation_id.clone(),
        observed_at_ms: 0,
    };
    agrees(&audit_record(directory, &expected.record_id), &expected).unwrap_or(false)
}

/// Write `selection` and then remove the marker. The marker stays when the
/// file could not be written.
#[allow(clippy::type_complexity)]
fn put_back(
    directory: &Path,
    selection: NativeHooksSelection,
    sync: DirectorySync,
) -> Result<Option<NativeHooksSelection>, (NativeHooksSelection, NativeHooksError)> {
    if write_choice(directory, &selection, sync).is_err() {
        return Err((selection, NativeHooksError::Storage));
    }
    clear_pending(directory, sync);
    Ok(Some(selection))
}

fn read_pending(path: &Path) -> Result<Option<PendingChoice>, ()> {
    let file = match open(path, OpenMode::Read) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(());
    }
    let pending: PendingChoice = serde_json::from_slice(&bytes).map_err(|_| ())?;
    // The record name becomes a path, so it must be one this store makes.
    let hex = pending
        .outcome
        .record_id
        .strip_prefix("native-hooks-")
        .ok_or(())?;
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        || !matches!(pending.outcome.target.agent.as_str(), "claude" | "codex")
    {
        return Err(());
    }
    Ok(Some(pending))
}

/// Best effort: a marker left behind names a change that is settled or
/// already put back, and the next start finds it so.
fn clear_pending(directory: &Path, sync: DirectorySync) {
    if std::fs::remove_file(directory.join(PENDING)).is_ok() {
        let _ = sync(directory);
    }
}

/// The file name of one phase. The same caller, harness, and request id
/// always name the same file, so a retry meets the record it already wrote.
fn record_name(
    agent: NativeHooksAgent,
    principal: &str,
    request: &str,
    phase: NativeHooksPhase,
) -> String {
    // A JSON array escapes every part, so ids that contain the separator
    // cannot spell another caller's tuple.
    let correlation = json!([agent.name(), principal, request, phase.name()]).to_string();
    let digest = Sha256::digest(correlation.as_bytes());
    format!("native-hooks-{digest:x}")
}

/// Where the durable audit adapter keeps this store's records.
fn audit_directory(directory: &Path) -> PathBuf {
    directory.join("audit").join("native-hooks")
}

fn audit_record(directory: &Path, name: &str) -> PathBuf {
    audit_directory(directory).join(format!("{name}.json"))
}

/// A record holds a caller id and a request id, each bounded by the protocol,
/// plus a few fixed fields. Reading stops past this, so a damaged or replaced
/// record fails the command instead of being held in memory whole.
const MAX_RECORD_BYTES: u64 = 64 * 1024;

fn read_record(path: &Path) -> Result<Option<StoredRecord>, NativeHooksError> {
    let Some(bytes) = read_bounded(path).map_err(|()| NativeHooksError::Audit)? else {
        return Ok(None);
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| NativeHooksError::Audit)
}

/// A record's bytes, or `Err` when the file could not be opened or read. A
/// file larger than any record comes back empty, which parses as nothing.
fn read_bounded(path: &Path) -> Result<Option<Vec<u8>>, ()> {
    let file = match open(path, OpenMode::Read) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        bytes.clear();
    }
    Ok(Some(bytes))
}

fn record_path(directory: &Path, change: &NativeHooksChange, phase: NativeHooksPhase) -> PathBuf {
    audit_record(
        directory,
        &record_name(
            change.agent,
            &change.principal_id,
            &change.request_id,
            phase,
        ),
    )
}

/// True when this request id already has an outcome for the same choice.
/// A stored outcome for a different choice is an audit failure: the id was reused.
fn already_recorded(
    directory: &Path,
    change: &NativeHooksChange,
) -> Result<bool, NativeHooksError> {
    let path = record_path(directory, change, NativeHooksPhase::Outcome);
    let Some(stored) = read_record(&path)? else {
        return Ok(false);
    };
    // The record must be the one this command writes, field for field. Only
    // the before value and the observation time come from the first attempt.
    let expected = StoredRecord::new(
        &change.evidence(
            NativeHooksPhase::Outcome,
            stored.transition.before,
            Some(change.enabled),
        ),
        stored.observed_at_ms,
    );
    if stored == expected {
        Ok(true)
    } else {
        Err(NativeHooksError::Audit)
    }
}

/// The before value already recorded for this request, when the requested
/// phase was written and the outcome was not.
fn recorded_before(
    directory: &Path,
    change: &NativeHooksChange,
) -> Result<Option<bool>, NativeHooksError> {
    let path = record_path(directory, change, NativeHooksPhase::Requested);
    let Some(stored) = read_record(&path)? else {
        return Ok(None);
    };
    let expected = StoredRecord::new(
        &change.evidence(NativeHooksPhase::Requested, stored.transition.before, None),
        stored.observed_at_ms,
    );
    if stored == expected {
        Ok(Some(stored.transition.before))
    } else {
        Err(NativeHooksError::Audit)
    }
}

/// One audit record as it is stored. Writing and reading share this shape,
/// so a record missing a field, carrying an extra one, or naming a field
/// twice does not read back as evidence.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredRecord {
    record_id: String,
    kind: String,
    target: StoredTarget,
    transition: StoredTransition,
    cause: String,
    initiator: StoredInitiator,
    correlation_id: String,
    observed_at_ms: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredTarget {
    agent: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredTransition {
    before: bool,
    requested: bool,
    after: Option<bool>,
    phase: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredInitiator {
    kind: String,
    principal_id: String,
    surface_id: String,
    organization_id: String,
}

impl StoredRecord {
    fn new(evidence: &NativeHooksEvidence, observed_at_ms: u64) -> Self {
        Self {
            record_id: record_name(
                evidence.agent,
                &evidence.principal_id,
                &evidence.request_id,
                evidence.phase,
            ),
            kind: "native_hooks".into(),
            target: StoredTarget {
                agent: evidence.agent.name().into(),
            },
            transition: StoredTransition {
                before: evidence.before,
                requested: evidence.requested,
                after: evidence.after,
                phase: evidence.phase.name().into(),
            },
            cause: "caller_requested".into(),
            initiator: StoredInitiator {
                kind: evidence.caller_kind.clone(),
                principal_id: evidence.principal_id.clone(),
                surface_id: evidence.surface_id.clone(),
                organization_id: evidence.organization_id.clone(),
            },
            correlation_id: evidence.request_id.clone(),
            observed_at_ms,
        }
    }
}

struct DurableNativeHooksAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
}

impl DurableNativeHooksAudit {
    fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> Result<Self, NativeHooksError> {
        create_directory(&directory).map_err(|_| NativeHooksError::Audit)?;
        Ok(Self { directory, clock })
    }
}

impl NativeHooksAudit for DurableNativeHooksAudit {
    fn record(&self, evidence: &NativeHooksEvidence) -> Result<(), NativeHooksError> {
        let record = StoredRecord::new(evidence, self.clock.unix_milliseconds());
        let destination = self.directory.join(format!("{}.json", record.record_id));
        if agrees(&destination, &record)? {
            return sync_directory(&self.directory).map_err(|_| NativeHooksError::Audit);
        }
        let mut file =
            PrivateTempFile::new_in(&self.directory).map_err(|_| NativeHooksError::Audit)?;
        serde_json::to_writer(file.as_file_mut(), &record).map_err(|_| NativeHooksError::Audit)?;
        file.as_file_mut()
            .write_all(b"\n")
            .and_then(|_| file.as_file().sync_all())
            .map_err(|_| NativeHooksError::Audit)?;
        if let Err(error) = file.publish(&destination) {
            if error.kind() != ErrorKind::AlreadyExists || !agrees(&destination, &record)? {
                return Err(NativeHooksError::Audit);
            }
        }
        sync_directory(&self.directory).map_err(|_| NativeHooksError::Audit)
    }
}

fn agrees(destination: &Path, expected: &StoredRecord) -> Result<bool, NativeHooksError> {
    let Some(stored) = read_record(destination)? else {
        return Ok(false);
    };
    // The observation time is assigned when the record is first written. A
    // retry builds a new time and still matches that record.
    Ok(StoredRecord {
        observed_at_ms: expected.observed_at_ms,
        ..stored
    } == *expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct FixedClock;
    impl Clock for FixedClock {
        fn unix_milliseconds(&self) -> u64 {
            1_700_000_000_000
        }
    }

    fn change(agent: NativeHooksAgent, enabled: bool, request_id: &str) -> NativeHooksChange {
        NativeHooksChange {
            agent,
            enabled,
            request_id: request_id.into(),
            organization_id: "organization".into(),
            principal_id: "principal".into(),
            surface_id: "credential".into(),
            caller_kind: "person".into(),
        }
    }

    #[derive(Default)]
    struct PhaseAudit {
        fail: Mutex<Option<NativeHooksPhase>>,
        records: Mutex<Vec<NativeHooksEvidence>>,
    }
    impl NativeHooksAudit for PhaseAudit {
        fn record(&self, evidence: &NativeHooksEvidence) -> Result<(), NativeHooksError> {
            if self.fail.lock().unwrap().as_ref() == Some(&evidence.phase) {
                return Err(NativeHooksError::Audit);
            }
            self.records.lock().unwrap().push(evidence.clone());
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_change_records_the_caller_and_a_retry_keeps_one_record_per_phase() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let preferences =
            NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        let selection = preferences
            .set(change(NativeHooksAgent::Claude, true, "request-1"))
            .await
            .unwrap();
        assert!(selection.claude());
        assert!(!selection.codex());
        assert!(preferences.switch(NativeHooksAgent::Claude).allowed());
        preferences
            .set(change(NativeHooksAgent::Claude, true, "request-1"))
            .await
            .unwrap();
        let audit = directory.join("audit").join("native-hooks");
        let records: Vec<_> = std::fs::read_dir(&audit)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        assert_eq!(records.len(), 2);
        let requested = records
            .iter()
            .map(|path| std::fs::read_to_string(path).unwrap())
            .find(|text| text.contains("\"phase\":\"requested\""))
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&requested).unwrap();
        assert_eq!(value["target"]["agent"], "claude");
        assert_eq!(value["transition"]["before"], false);
        assert_eq!(value["transition"]["requested"], true);
        assert!(value["transition"]["after"].is_null());
        assert_eq!(value["cause"], "caller_requested");
        assert_eq!(value["initiator"]["kind"], "person");
        assert_eq!(value["initiator"]["principalId"], "principal");
        assert_eq!(value["initiator"]["surfaceId"], "credential");
        assert_eq!(value["initiator"]["organizationId"], "organization");
        assert_eq!(value["correlationId"], "request-1");
        assert_eq!(value["observedAtMs"], 1_700_000_000_000u64);
        let stored = std::fs::read_to_string(directory.join("native-hooks.json")).unwrap();
        assert_eq!(
            selection_from_stored(stored.as_bytes()),
            NativeHooksSelection::new(true, false)
        );
    }

    struct PanickingAudit;
    impl NativeHooksAudit for PanickingAudit {
        fn record(&self, _evidence: &NativeHooksEvidence) -> Result<(), NativeHooksError> {
            panic!("save task crashed");
        }
    }

    #[tokio::test]
    async fn a_crashed_save_is_not_an_audit_failure() {
        let root = tempfile::tempdir().unwrap();
        let preferences =
            NativeHooksPreferences::open_with(root.path().join("data"), Arc::new(PanickingAudit))
                .unwrap();
        let error = preferences
            .set(change(NativeHooksAgent::Claude, true, "request-crash"))
            .await
            .unwrap_err();
        assert_eq!(error, NativeHooksError::Interrupted);
    }

    #[tokio::test]
    async fn a_failed_requested_record_leaves_the_choice_off() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let audit = Arc::new(PhaseAudit {
            fail: Mutex::new(Some(NativeHooksPhase::Requested)),
            records: Mutex::new(Vec::new()),
        });
        let preferences = NativeHooksPreferences::open_with(directory.clone(), audit).unwrap();
        let error = preferences
            .set(change(NativeHooksAgent::Codex, true, "request-2"))
            .await
            .unwrap_err();
        assert_eq!(error, NativeHooksError::Audit);
        assert!(!preferences.selection().codex());
        assert!(!directory.join("native-hooks.json").exists());
    }

    #[tokio::test]
    async fn a_failed_outcome_record_reverts_the_choice() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let audit = Arc::new(PhaseAudit {
            fail: Mutex::new(Some(NativeHooksPhase::Outcome)),
            records: Mutex::new(Vec::new()),
        });
        let preferences =
            NativeHooksPreferences::open_with(directory.clone(), audit.clone()).unwrap();
        let error = preferences
            .set(change(NativeHooksAgent::Codex, true, "request-3"))
            .await
            .unwrap_err();
        assert_eq!(error, NativeHooksError::Audit);
        assert!(!preferences.switch(NativeHooksAgent::Codex).allowed());
        let stored = std::fs::read_to_string(directory.join("native-hooks.json")).unwrap();
        assert!(!selection_from_stored(stored.as_bytes()).codex());
        assert_eq!(audit.records.lock().unwrap().len(), 1);
        assert_eq!(
            audit.records.lock().unwrap()[0].phase,
            NativeHooksPhase::Requested
        );
    }

    #[tokio::test]
    async fn an_invalid_file_loads_as_off() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let preferences =
            NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        preferences
            .set(change(NativeHooksAgent::Claude, true, "seed"))
            .await
            .unwrap();
        let path = directory.join("native-hooks.json");
        // Keep the private mode the store gave the file. A public mode is a
        // different case: it loads as off, and it does not stop startup.
        std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap()
            .write_all(b"{")
            .unwrap();
        let loaded = NativeHooksPreferences::open(directory, Arc::new(FixedClock)).unwrap();
        assert_eq!(loaded.selection(), NativeHooksSelection::off());
    }

    struct SteppingClock {
        next: std::sync::atomic::AtomicU64,
    }
    impl Clock for SteppingClock {
        fn unix_milliseconds(&self) -> u64 {
            self.next.fetch_add(1, Ordering::SeqCst)
        }
    }

    struct FailFirstOutcome {
        inner: DurableNativeHooksAudit,
        failed: std::sync::atomic::AtomicBool,
    }
    impl NativeHooksAudit for FailFirstOutcome {
        fn record(&self, evidence: &NativeHooksEvidence) -> Result<(), NativeHooksError> {
            if evidence.phase == NativeHooksPhase::Outcome
                && self
                    .failed
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                return Err(NativeHooksError::Audit);
            }
            self.inner.record(evidence)
        }
    }

    #[tokio::test]
    async fn a_retry_after_a_failed_outcome_keeps_the_first_observation() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let clock = Arc::new(SteppingClock {
            next: std::sync::atomic::AtomicU64::new(10),
        });
        let audit = Arc::new(FailFirstOutcome {
            inner: DurableNativeHooksAudit::new(
                directory.join("audit").join("native-hooks"),
                clock,
            )
            .unwrap(),
            failed: std::sync::atomic::AtomicBool::new(false),
        });
        let preferences = NativeHooksPreferences::open_with(directory.clone(), audit).unwrap();
        let error = preferences
            .set(change(NativeHooksAgent::Claude, true, "request-4"))
            .await
            .unwrap_err();
        assert_eq!(error, NativeHooksError::Audit);
        assert!(!preferences.selection().claude());
        let selection = preferences
            .set(change(NativeHooksAgent::Claude, true, "request-4"))
            .await
            .unwrap();
        assert!(selection.claude());
        let audit_dir = directory.join("audit").join("native-hooks");
        let requested = std::fs::read_dir(&audit_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .map(|path| std::fs::read_to_string(path).unwrap())
            .find(|text| text.contains("\"phase\":\"requested\""))
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&requested).unwrap();
        assert_eq!(value["observedAtMs"], 10);
        assert_eq!(value["transition"]["before"], false);
        assert_eq!(value["transition"]["requested"], true);
        assert!(value["transition"]["after"].is_null());
    }

    /// Syncs the pending-change marker, then fails every directory sync after
    /// it. Only `a_choice_renamed_into_place_is_in_effect_when_its_directory_sync_fails`
    /// uses it, so the count is that test's own.
    fn unsynced(path: &Path) -> std::io::Result<()> {
        static SYNCS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        if SYNCS.fetch_add(1, Ordering::SeqCst) == 0 {
            return sync_directory(path);
        }
        Err(std::io::Error::other("directory sync failed"))
    }

    #[tokio::test]
    async fn a_choice_renamed_into_place_is_in_effect_when_its_directory_sync_fails() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let audit = Arc::new(
            DurableNativeHooksAudit::new(
                directory.join("audit").join("native-hooks"),
                Arc::new(FixedClock),
            )
            .unwrap(),
        );
        let preferences =
            NativeHooksPreferences::open_syncing(directory.clone(), audit, unsynced).unwrap();
        let error = preferences
            .set(change(NativeHooksAgent::Claude, true, "request-unsynced"))
            .await
            .unwrap_err();
        assert_eq!(error, NativeHooksError::Storage);
        let stored = std::fs::read_to_string(directory.join("native-hooks.json")).unwrap();
        assert!(selection_from_stored(stored.as_bytes()).claude());
        // The rename could still be lost, so the change stays pending.
        assert!(directory.join(PENDING).exists());
        // The running switch agrees with the file the next start will load.
        assert!(preferences.selection().claude());
        assert!(preferences.switch(NativeHooksAgent::Claude).allowed());
        let reloaded =
            NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        assert_eq!(reloaded.selection(), preferences.selection());
        // A retry is not acknowledged until the directory sync succeeds.
        let retry = change(NativeHooksAgent::Claude, true, "request-unsynced");
        assert_eq!(
            preferences.set(retry.clone()).await.unwrap_err(),
            NativeHooksError::Storage
        );
        assert!(reloaded.set(retry).await.unwrap().claude());
    }

    /// Publishes every record, then reports the outcome record as failed: the
    /// record is readable, but its directory sync was not confirmed.
    struct OutcomeUnconfirmed {
        inner: DurableNativeHooksAudit,
    }
    impl NativeHooksAudit for OutcomeUnconfirmed {
        fn record(&self, evidence: &NativeHooksEvidence) -> Result<(), NativeHooksError> {
            self.inner.record(evidence)?;
            if evidence.phase == NativeHooksPhase::Outcome {
                return Err(NativeHooksError::Audit);
            }
            Ok(())
        }
    }

    /// Publishes the outcome record, then reports a failure and leaves the
    /// record unreadable: a directory takes its place.
    struct OutcomeUnreadable {
        inner: DurableNativeHooksAudit,
        directory: PathBuf,
    }
    impl NativeHooksAudit for OutcomeUnreadable {
        fn record(&self, evidence: &NativeHooksEvidence) -> Result<(), NativeHooksError> {
            self.inner.record(evidence)?;
            if evidence.phase == NativeHooksPhase::Outcome {
                let path = audit_record(&self.directory, &StoredRecord::new(evidence, 0).record_id);
                std::fs::remove_file(&path).unwrap();
                std::fs::create_dir(&path).unwrap();
                return Err(NativeHooksError::Audit);
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn an_outcome_that_cannot_be_read_back_keeps_the_change_pending_and_the_harness_off() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let audit = Arc::new(OutcomeUnreadable {
            inner: DurableNativeHooksAudit::new(audit_directory(&directory), Arc::new(FixedClock))
                .unwrap(),
            directory: directory.clone(),
        });
        let preferences = NativeHooksPreferences::open_with(directory.clone(), audit).unwrap();
        assert_eq!(
            preferences
                .set(change(
                    NativeHooksAgent::Claude,
                    true,
                    "request-unreadable-outcome"
                ))
                .await
                .unwrap_err(),
            NativeHooksError::Audit
        );
        assert!(!preferences.selection().claude());
        assert!(directory.join(PENDING).exists());
        let stored = std::fs::read_to_string(directory.join(CHOICE)).unwrap();
        assert!(
            selection_from_stored(stored.as_bytes()).claude(),
            "not reverted"
        );
    }

    #[tokio::test]
    async fn a_readable_outcome_record_keeps_the_change_it_records() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let audit = Arc::new(OutcomeUnconfirmed {
            inner: DurableNativeHooksAudit::new(
                directory.join("audit").join("native-hooks"),
                Arc::new(FixedClock),
            )
            .unwrap(),
        });
        let preferences = NativeHooksPreferences::open_with(directory.clone(), audit).unwrap();
        let error = preferences
            .set(change(NativeHooksAgent::Codex, true, "request-unconfirmed"))
            .await
            .unwrap_err();
        assert_eq!(error, NativeHooksError::Audit);
        assert!(preferences.selection().codex());
        let stored = std::fs::read_to_string(directory.join("native-hooks.json")).unwrap();
        assert!(selection_from_stored(stored.as_bytes()).codex());
        // The record's directory is not confirmed, so the marker stays. A
        // start that cannot sync that directory keeps both.
        assert!(directory.join(PENDING).exists());
        let restarted = NativeHooksPreferences::open_syncing(
            directory.clone(),
            Arc::new(PhaseAudit::default()),
            audit_unsynced,
        )
        .unwrap();
        assert!(restarted.selection().codex());
        assert!(directory.join(PENDING).exists());
        // The retry syncs it, meets the outcome record, and reports the
        // choice it records.
        let selection = preferences
            .set(change(NativeHooksAgent::Codex, true, "request-unconfirmed"))
            .await
            .unwrap();
        assert!(selection.codex());
        assert!(!directory.join(PENDING).exists());
    }

    #[tokio::test]
    async fn an_audit_record_larger_than_any_record_fails_the_retry() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let preferences =
            NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        let retry = change(NativeHooksAgent::Claude, true, "request-oversized");
        preferences.set(retry.clone()).await.unwrap();
        let outcome = record_path(&directory, &retry, NativeHooksPhase::Outcome);
        // Still the same valid record, padded past what any record needs.
        let text = std::fs::read_to_string(&outcome).unwrap();
        let padded = format!("{}{}", " ".repeat(MAX_RECORD_BYTES as usize), text);
        std::fs::write(&outcome, padded).unwrap();
        assert_eq!(
            preferences.set(retry).await.unwrap_err(),
            NativeHooksError::Audit
        );
    }

    #[tokio::test]
    async fn an_outcome_record_missing_its_attribution_is_not_evidence() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let preferences =
            NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        let retry = change(NativeHooksAgent::Claude, true, "request-bare");
        preferences.set(retry.clone()).await.unwrap();
        let outcome = record_path(&directory, &retry, NativeHooksPhase::Outcome);
        // The four fields a retry once looked at, and nothing that says who
        // asked, when, or from what.
        std::fs::write(
            &outcome,
            r#"{"target":{"agent":"claude"},"transition":{"requested":true,"after":true},"correlationId":"request-bare"}"#,
        )
        .unwrap();
        assert_eq!(
            preferences.set(retry).await.unwrap_err(),
            NativeHooksError::Audit
        );
    }

    #[test]
    fn ids_that_contain_the_separator_name_separate_records() {
        assert_ne!(
            record_name(
                NativeHooksAgent::Claude,
                "a:b",
                "c",
                NativeHooksPhase::Outcome
            ),
            record_name(
                NativeHooksAgent::Claude,
                "a",
                "b:c",
                NativeHooksPhase::Outcome
            ),
        );
    }

    /// Stops the command where a gateway crash would: after the file is
    /// published and before its outcome is recorded.
    struct StopsBeforeTheOutcome {
        inner: DurableNativeHooksAudit,
    }
    impl NativeHooksAudit for StopsBeforeTheOutcome {
        fn record(&self, evidence: &NativeHooksEvidence) -> Result<(), NativeHooksError> {
            if evidence.phase == NativeHooksPhase::Outcome {
                panic!("the gateway stopped");
            }
            self.inner.record(evidence)
        }
    }

    #[tokio::test]
    async fn a_choice_published_without_its_outcome_is_put_back_at_the_next_start() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let crashed = NativeHooksPreferences::open_with(
            directory.clone(),
            Arc::new(StopsBeforeTheOutcome {
                inner: DurableNativeHooksAudit::new(
                    directory.join("audit").join("native-hooks"),
                    Arc::new(FixedClock),
                )
                .unwrap(),
            }),
        )
        .unwrap();
        let retry = change(NativeHooksAgent::Codex, true, "request-crash");
        assert_eq!(
            crashed.set(retry.clone()).await.unwrap_err(),
            NativeHooksError::Interrupted
        );
        let stored = std::fs::read_to_string(directory.join("native-hooks.json")).unwrap();
        assert!(
            selection_from_stored(stored.as_bytes()).codex(),
            "published"
        );
        // The next start loads the choice the audit last recorded, not the file.
        let restarted =
            NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        assert_eq!(restarted.selection(), NativeHooksSelection::off());
        let stored = std::fs::read_to_string(directory.join("native-hooks.json")).unwrap();
        assert!(
            !selection_from_stored(stored.as_bytes()).codex(),
            "put back"
        );
        assert!(!directory.join(PENDING).exists());
        // The same command can then finish.
        assert!(restarted.set(retry).await.unwrap().codex());
        assert!(!directory.join(PENDING).exists());
    }

    #[tokio::test]
    async fn a_pending_change_whose_outcome_record_contradicts_it_is_put_back() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let crashed = NativeHooksPreferences::open_with(
            directory.clone(),
            Arc::new(StopsBeforeTheOutcome {
                inner: DurableNativeHooksAudit::new(
                    directory.join("audit").join("native-hooks"),
                    Arc::new(FixedClock),
                )
                .unwrap(),
            }),
        )
        .unwrap();
        let turned_on = change(NativeHooksAgent::Claude, true, "request-contradicted");
        crashed.set(turned_on.clone()).await.unwrap_err();
        // A complete record under the outcome's name that records a different
        // change: the same id and phase, but Claude turned off.
        let contradicting = StoredRecord::new(
            &turned_on.evidence(NativeHooksPhase::Outcome, false, Some(false)),
            1,
        );
        let name = format!("{}.json", contradicting.record_id);
        publish(
            &audit_directory(&directory),
            &name,
            &contradicting,
            sync_directory,
        )
        .unwrap();
        let restarted =
            NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        assert_eq!(restarted.selection(), NativeHooksSelection::off());
        let stored = std::fs::read_to_string(directory.join(CHOICE)).unwrap();
        assert!(!selection_from_stored(stored.as_bytes()).claude());
        assert!(!directory.join(PENDING).exists());
    }

    #[tokio::test]
    async fn an_unreadable_outcome_keeps_the_change_pending_and_the_harness_off() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let preferences =
            NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        let done = change(NativeHooksAgent::Claude, true, "request-unreadable");
        preferences.set(done.clone()).await.unwrap();
        let outcome = StoredRecord::new(
            &done.evidence(NativeHooksPhase::Outcome, false, Some(true)),
            0,
        );
        let path = audit_record(&directory, &outcome.record_id);
        publish(
            &directory,
            PENDING,
            &PendingChoice {
                outcome,
                before: false,
            },
            sync_directory,
        )
        .unwrap();
        // The outcome is there, but this start cannot read it.
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let restarted =
            NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        assert!(!restarted.selection().claude());
        assert!(directory.join(PENDING).exists());
        let stored = std::fs::read_to_string(directory.join(CHOICE)).unwrap();
        assert!(
            selection_from_stored(stored.as_bytes()).claude(),
            "neither kept nor put back"
        );
        assert_eq!(
            restarted
                .set(change(NativeHooksAgent::Codex, true, "request-meanwhile"))
                .await
                .unwrap_err(),
            NativeHooksError::Audit
        );
    }

    /// Syncs the marker and the new file, then fails every later sync of the
    /// choice's directory: the revert renames into place but does not sync.
    fn revert_unsynced(path: &Path) -> std::io::Result<()> {
        static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        if !path.ends_with("audit/native-hooks") && CALLS.fetch_add(1, Ordering::SeqCst) >= 2 {
            return Err(std::io::Error::other("directory sync failed"));
        }
        sync_directory(path)
    }

    #[tokio::test]
    async fn a_revert_that_does_not_sync_keeps_the_change_pending() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let preferences = NativeHooksPreferences::open_syncing(
            directory.clone(),
            Arc::new(PhaseAudit {
                fail: Mutex::new(Some(NativeHooksPhase::Outcome)),
                records: Mutex::new(Vec::new()),
            }),
            revert_unsynced,
        )
        .unwrap();
        assert_eq!(
            preferences
                .set(change(
                    NativeHooksAgent::Codex,
                    true,
                    "request-unsynced-revert"
                ))
                .await
                .unwrap_err(),
            NativeHooksError::Audit
        );
        assert!(!preferences.selection().codex());
        // A crash could still undo the rename, so the marker that puts the
        // file back stays.
        assert!(directory.join(PENDING).exists());
    }

    #[tokio::test]
    async fn a_recorded_change_whose_file_rename_was_lost_is_written_again() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let preferences =
            NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        preferences
            .set(change(NativeHooksAgent::Claude, true, "request-on"))
            .await
            .unwrap();
        let off = change(NativeHooksAgent::Claude, false, "request-off");
        preferences.set(off.clone()).await.unwrap();
        // A crash undid the rename of the off file and the marker's removal.
        // The outcome record, which synced, says off.
        write_choice(
            &directory,
            &NativeHooksSelection::new(true, false),
            sync_directory,
        )
        .unwrap();
        let pending = PendingChoice {
            outcome: StoredRecord::new(
                &off.evidence(NativeHooksPhase::Outcome, true, Some(false)),
                0,
            ),
            before: true,
        };
        publish(&directory, PENDING, &pending, sync_directory).unwrap();
        let restarted =
            NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        assert!(!restarted.selection().claude());
        let stored = std::fs::read_to_string(directory.join(CHOICE)).unwrap();
        assert!(!selection_from_stored(stored.as_bytes()).claude());
        assert!(!directory.join(PENDING).exists());
    }

    #[tokio::test]
    async fn hooks_turned_off_by_a_crashed_command_go_back_on_only_with_its_requested_record() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let preferences =
            NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        preferences
            .set(change(NativeHooksAgent::Claude, true, "request-on"))
            .await
            .unwrap();
        drop(preferences);
        let crashed = NativeHooksPreferences::open_with(
            directory.clone(),
            Arc::new(StopsBeforeTheOutcome {
                inner: DurableNativeHooksAudit::new(
                    audit_directory(&directory),
                    Arc::new(FixedClock),
                )
                .unwrap(),
            }),
        )
        .unwrap();
        crashed
            .set(change(NativeHooksAgent::Claude, false, "request-off"))
            .await
            .unwrap_err();
        // The requested record says hooks were on, so they go back on.
        let restarted =
            NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        assert!(restarted.selection().claude());
        // A marker that claims hooks were on, with no requested record behind
        // it, puts them back off.
        let forged = change(NativeHooksAgent::Codex, false, "request-forged");
        publish(
            &directory,
            PENDING,
            &PendingChoice {
                outcome: StoredRecord::new(
                    &forged.evidence(NativeHooksPhase::Outcome, true, Some(false)),
                    0,
                ),
                before: true,
            },
            sync_directory,
        )
        .unwrap();
        let forged_start =
            NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        assert!(!forged_start.selection().codex());
        assert!(forged_start.selection().claude());
        assert!(!directory.join(PENDING).exists());
    }

    #[tokio::test]
    async fn a_pending_change_whose_outcome_was_recorded_is_kept_at_the_next_start() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let preferences =
            NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        let done = change(NativeHooksAgent::Claude, true, "request-settled");
        preferences.set(done.clone()).await.unwrap();
        // A gateway that stopped after the outcome and before clearing the
        // marker leaves this behind.
        let pending = PendingChoice {
            outcome: StoredRecord::new(
                &done.evidence(NativeHooksPhase::Outcome, false, Some(true)),
                0,
            ),
            before: false,
        };
        publish(&directory, PENDING, &pending, sync_directory).unwrap();
        let restarted =
            NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        assert!(restarted.selection().claude());
        assert!(!directory.join(PENDING).exists());
    }

    #[tokio::test]
    async fn an_unreadable_pending_change_loads_as_off() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let preferences =
            NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        preferences
            .set(change(
                NativeHooksAgent::Claude,
                true,
                "request-before-damage",
            ))
            .await
            .unwrap();
        let mut outcome = StoredRecord::new(
            &change(NativeHooksAgent::Claude, true, "request-before-damage").evidence(
                NativeHooksPhase::Outcome,
                false,
                Some(true),
            ),
            0,
        );
        outcome.record_id = "../../elsewhere".into();
        let pending = PendingChoice {
            outcome,
            before: false,
        };
        publish(&directory, PENDING, &pending, sync_directory).unwrap();
        let restarted =
            NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        assert_eq!(restarted.selection(), NativeHooksSelection::off());
        // The file is written off and the marker goes, so a later start stays
        // off and the next command is not refused.
        assert!(!directory.join(PENDING).exists());
        let again = NativeHooksPreferences::open(directory, Arc::new(FixedClock)).unwrap();
        assert_eq!(again.selection(), NativeHooksSelection::off());
        assert!(again
            .set(change(
                NativeHooksAgent::Codex,
                true,
                "request-after-damage"
            ))
            .await
            .unwrap()
            .codex());
    }

    fn audit_unsynced(path: &Path) -> std::io::Result<()> {
        if path.ends_with("audit/native-hooks") {
            return Err(std::io::Error::other("audit directory sync failed"));
        }
        sync_directory(path)
    }

    #[tokio::test]
    async fn a_retry_is_not_acknowledged_until_the_outcome_records_directory_syncs() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let first = NativeHooksPreferences::open_with(
            directory.clone(),
            Arc::new(OutcomeUnconfirmed {
                inner: DurableNativeHooksAudit::new(
                    directory.join("audit").join("native-hooks"),
                    Arc::new(FixedClock),
                )
                .unwrap(),
            }),
        )
        .unwrap();
        let retry = change(NativeHooksAgent::Codex, true, "request-audit-unsynced");
        assert_eq!(
            first.set(retry.clone()).await.unwrap_err(),
            NativeHooksError::Audit
        );
        let audit = Arc::new(
            DurableNativeHooksAudit::new(
                directory.join("audit").join("native-hooks"),
                Arc::new(FixedClock),
            )
            .unwrap(),
        );
        let unsynced =
            NativeHooksPreferences::open_syncing(directory.clone(), audit, audit_unsynced).unwrap();
        assert_eq!(
            unsynced.set(retry.clone()).await.unwrap_err(),
            NativeHooksError::Audit
        );
        let synced = NativeHooksPreferences::open(directory, Arc::new(FixedClock)).unwrap();
        assert!(synced.set(retry).await.unwrap().codex());
    }

    /// Notes what a process start would read while the outcome is recorded.
    struct SeesTheSwitch {
        switch: Mutex<Option<NativeHooksSwitch>>,
        seen: Mutex<Option<bool>>,
        fail: bool,
    }
    impl NativeHooksAudit for SeesTheSwitch {
        fn record(&self, evidence: &NativeHooksEvidence) -> Result<(), NativeHooksError> {
            if evidence.phase == NativeHooksPhase::Outcome {
                let allowed = self.switch.lock().unwrap().as_ref().unwrap().allowed();
                *self.seen.lock().unwrap() = Some(allowed);
                if self.fail {
                    return Err(NativeHooksError::Audit);
                }
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_process_start_reads_the_earlier_choice_until_the_outcome_is_recorded() {
        for fail in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let audit = Arc::new(SeesTheSwitch {
                switch: Mutex::new(None),
                seen: Mutex::new(None),
                fail,
            });
            let preferences =
                NativeHooksPreferences::open_with(root.path().join("data"), audit.clone()).unwrap();
            *audit.switch.lock().unwrap() = Some(preferences.switch(NativeHooksAgent::Codex));
            let result = preferences
                .set(change(NativeHooksAgent::Codex, true, "request-ordered"))
                .await;
            assert_eq!(*audit.seen.lock().unwrap(), Some(false));
            assert_eq!(result.is_ok(), !fail);
            assert_eq!(preferences.selection().codex(), !fail);
        }
    }

    #[tokio::test]
    async fn a_reused_request_id_for_a_different_choice_fails() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let preferences =
            NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        preferences
            .set(change(NativeHooksAgent::Codex, true, "request-5"))
            .await
            .unwrap();
        let error = preferences
            .set(change(NativeHooksAgent::Codex, false, "request-5"))
            .await
            .unwrap_err();
        assert_eq!(error, NativeHooksError::Audit);
        assert!(preferences.selection().codex());
        let stored = std::fs::read_to_string(directory.join("native-hooks.json")).unwrap();
        assert!(selection_from_stored(stored.as_bytes()).codex());
        let records = std::fs::read_dir(directory.join("audit").join("native-hooks")).unwrap();
        assert_eq!(records.count(), 2);
    }

    #[tokio::test]
    async fn a_file_without_an_outcome_loads_as_written() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let preferences =
            NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        preferences
            .set(change(NativeHooksAgent::Claude, true, "request-6"))
            .await
            .unwrap();
        let audit_dir = directory.join("audit").join("native-hooks");
        for entry in std::fs::read_dir(&audit_dir).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            if text.contains("\"phase\":\"outcome\"") {
                std::fs::remove_file(path).unwrap();
            }
        }
        let loaded = NativeHooksPreferences::open(directory, Arc::new(FixedClock)).unwrap();
        assert!(loaded.selection().claude());
        assert!(!loaded.selection().codex());
    }

    #[tokio::test]
    async fn an_agent_caller_is_recorded_as_an_agent() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let preferences =
            NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        let mut requested = change(NativeHooksAgent::Codex, true, "request-agent");
        requested.caller_kind = "agent".into();
        preferences.set(requested).await.unwrap();
        let audit = directory.join("audit").join("native-hooks");
        let recorded = std::fs::read_dir(&audit)
            .unwrap()
            .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
            .find(|text| text.contains("\"phase\":\"requested\""))
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&recorded).unwrap();
        assert_eq!(value["initiator"]["kind"], "agent");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn review_a_public_mode_choice_file_loads_as_off() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        let path = directory.join("native-hooks.json");
        std::fs::write(&path, br#"{"claude":true,"codex":true}"#).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let loaded = NativeHooksPreferences::open(directory, Arc::new(FixedClock)).unwrap();
        assert_eq!(loaded.selection(), NativeHooksSelection::off());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_choice_file_larger_than_any_choice_loads_as_off() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        let path = directory.join("native-hooks.json");
        // Valid JSON that would turn both on if it were read whole.
        let padded = format!(
            r#"{{"claude":true,{}"codex":true}}"#,
            " ".repeat(MAX_CHOICE_BYTES as usize)
        );
        std::fs::write(&path, padded).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let loaded = NativeHooksPreferences::open(directory, Arc::new(FixedClock)).unwrap();
        assert_eq!(loaded.selection(), NativeHooksSelection::off());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn review_a_symlinked_choice_file_loads_as_off() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        let real = directory.join("elsewhere.json");
        std::fs::write(&real, br#"{"claude":true,"codex":true}"#).unwrap();
        std::os::unix::fs::symlink(&real, directory.join("native-hooks.json")).unwrap();
        let loaded = NativeHooksPreferences::open(directory, Arc::new(FixedClock)).unwrap();
        assert_eq!(loaded.selection(), NativeHooksSelection::off());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn review_a_directory_in_place_of_the_choice_file_loads_as_off() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        NativeHooksPreferences::open(directory.clone(), Arc::new(FixedClock)).unwrap();
        std::fs::create_dir(directory.join("native-hooks.json")).unwrap();
        let loaded = NativeHooksPreferences::open(directory, Arc::new(FixedClock)).unwrap();
        assert_eq!(loaded.selection(), NativeHooksSelection::off());
    }

    #[cfg(unix)]
    struct LockOnOutcome {
        directory: PathBuf,
        locked: AtomicBool,
    }
    #[cfg(unix)]
    impl NativeHooksAudit for LockOnOutcome {
        fn record(&self, evidence: &NativeHooksEvidence) -> Result<(), NativeHooksError> {
            use std::os::unix::fs::PermissionsExt;
            // Only the first outcome fails.
            if evidence.phase == NativeHooksPhase::Outcome
                && !self.locked.swap(true, Ordering::SeqCst)
            {
                std::fs::set_permissions(&self.directory, std::fs::Permissions::from_mode(0o500))
                    .unwrap();
                return Err(NativeHooksError::Audit);
            }
            Ok(())
        }
    }

    #[cfg(unix)]
    struct RestoreMode(PathBuf);
    #[cfg(unix)]
    impl Drop for RestoreMode {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_revert_keeps_the_recorded_flag_and_the_next_command_puts_the_file_back() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let audit = Arc::new(LockOnOutcome {
            directory: directory.clone(),
            locked: AtomicBool::new(false),
        });
        let preferences = NativeHooksPreferences::open_with(directory.clone(), audit).unwrap();
        let _restore = RestoreMode(directory.clone());
        let error = preferences
            .set(change(NativeHooksAgent::Codex, true, "request-7"))
            .await
            .unwrap_err();
        assert_eq!(error, NativeHooksError::Audit);
        // No outcome was recorded, so process starts keep the earlier choice.
        assert!(!preferences.selection().codex());
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        let stored = std::fs::read_to_string(directory.join("native-hooks.json")).unwrap();
        assert!(selection_from_stored(stored.as_bytes()).codex());
        assert!(directory.join(PENDING).exists());
        // The next command, for the other harness, first puts Codex back:
        // its own marker must not replace one that still has work to do.
        let selection = preferences
            .set(change(NativeHooksAgent::Claude, true, "request-8"))
            .await
            .unwrap();
        assert_eq!(selection, NativeHooksSelection::new(true, false));
        let stored = std::fs::read_to_string(directory.join(CHOICE)).unwrap();
        assert_eq!(
            selection_from_stored(stored.as_bytes()),
            NativeHooksSelection::new(true, false)
        );
        assert!(!directory.join(PENDING).exists());
    }
}
