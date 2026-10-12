//! The native-hooks choice file and its audit records.
//!
//! ```text
//! set -> requested record -> native-hooks.json -> atomic flag -> outcome record
//! open -> native-hooks.json -> flags (unreadable or invalid documents load as off)
//! process start -> switch.allowed() once -> that start's profile and environment
//! ```
//!
//! Arrows are the order of one change, of one startup read, and of one
//! process start. A failed requested record writes nothing. A failed
//! outcome record reverts the file and the flag. A retry of the same
//! principal, harness, and request id keeps the record already written,
//! including its observation time, when the choice matches, and fails when
//! that id was used for a different choice. The adapter assigns the record id
//! and the observation time. There is no provider process in this transition:
//! each process start reads the flag once, including the next start of a
//! conversation already open. A process already running is not changed.
//!
//! A crash after the file is published and before the outcome record leaves
//! the choice on disk with the requested record and no outcome record. The
//! next start loads the file. That window is the remaining gap.

use crate::agents::application::{
    selection_from_stored, NativeHooksAgent, NativeHooksAudit, NativeHooksChange, NativeHooksError,
    NativeHooksEvidence, NativeHooksPhase, NativeHooksSelection,
};
use nessa_auth::application::ports::Clock;
use nessa_local_storage::{create_directory, open, sync_directory, OpenMode, PrivateTempFile};
use nessa_sdk::application::agent_execution::providers::NativeHooksSwitch;
use serde_json::{json, Value};
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
}

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
        create_directory(&directory).map_err(|_| NativeHooksError::Storage)?;
        let stored = read_choice(&directory.join("native-hooks.json"));
        Ok(Arc::new(Self {
            directory,
            claude: Arc::new(AtomicBool::new(stored.claude())),
            codex: Arc::new(AtomicBool::new(stored.codex())),
            gate: Arc::new(Mutex::new(())),
            audit,
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
    /// returns an error. An outcome-record failure puts the flag back. When
    /// the file can be written, it goes back too. When that write fails, the
    /// flag stays with the file that was published.
    pub async fn set(
        &self,
        change: NativeHooksChange,
    ) -> Result<NativeHooksSelection, NativeHooksError> {
        let directory = self.directory.clone();
        let claude = self.claude.clone();
        let codex = self.codex.clone();
        let audit = self.audit.clone();
        let gate = self.gate.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = gate.lock().unwrap_or_else(|error| error.into_inner());
            set_blocking(directory, claude, codex, audit, change)
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
    change: NativeHooksChange,
) -> Result<NativeHooksSelection, NativeHooksError> {
    // A retry of a command that already recorded its outcome keeps that
    // record. The before value in it is the one from the first attempt.
    if already_recorded(&directory, &change)? {
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
    write_choice(&directory, &updated)?;
    store(&claude, &codex, change.agent, change.enabled);
    if let Err(error) =
        audit.record(&change.evidence(NativeHooksPhase::Outcome, before, Some(change.enabled)))
    {
        store(&claude, &codex, change.agent, current);
        let reverted = selection_of(&claude, &codex);
        if write_choice(&directory, &reverted).is_err() {
            store(&claude, &codex, change.agent, change.enabled);
        }
        return Err(error);
    }
    Ok(selection_of(&claude, &codex))
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

/// A choice file that cannot be opened or read is off. A missing file is the
/// first start and stays quiet. Every other failure is logged: off is the
/// safe direction, and it must not stop the gateway.
fn read_choice(path: &Path) -> NativeHooksSelection {
    let mut file = match open(path, OpenMode::Read) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return NativeHooksSelection::off(),
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "native hooks choice file could not be opened; loading as off");
            return NativeHooksSelection::off();
        }
    };
    let mut bytes = Vec::new();
    if let Err(error) = file.read_to_end(&mut bytes) {
        tracing::warn!(%error, path = %path.display(), "native hooks choice file could not be read; loading as off");
        return NativeHooksSelection::off();
    }
    selection_from_stored(&bytes)
}

fn write_choice(
    directory: &Path,
    selection: &NativeHooksSelection,
) -> Result<(), NativeHooksError> {
    publish_json(
        directory,
        &directory.join("native-hooks.json"),
        &json!({"claude": selection.claude(), "codex": selection.codex()}),
    )
}

fn publish_json(
    directory: &Path,
    destination: &Path,
    value: &Value,
) -> Result<(), NativeHooksError> {
    let mut file = PrivateTempFile::new_in(directory).map_err(|_| NativeHooksError::Storage)?;
    serde_json::to_writer(file.as_file_mut(), value).map_err(|_| NativeHooksError::Storage)?;
    file.as_file_mut()
        .write_all(b"\n")
        .and_then(|_| file.as_file().sync_all())
        .map_err(|_| NativeHooksError::Storage)?;
    file.persist(destination)
        .map_err(|_| NativeHooksError::Storage)?;
    sync_directory(directory).map_err(|_| NativeHooksError::Storage)
}

/// The file name of one phase. The same caller, harness, and request id
/// always name the same file, so a retry meets the record it already wrote.
fn record_name(
    agent: NativeHooksAgent,
    principal: &str,
    request: &str,
    phase: NativeHooksPhase,
) -> String {
    let correlation = format!("{}:{principal}:{request}:{}", agent.name(), phase.name());
    let digest = Sha256::digest(correlation.as_bytes());
    format!("native-hooks-{digest:x}")
}

fn audit_record(directory: &Path, name: &str) -> PathBuf {
    directory
        .join("audit")
        .join("native-hooks")
        .join(format!("{name}.json"))
}

fn read_record(path: &Path) -> Result<Option<Value>, NativeHooksError> {
    let mut file = match open(path, OpenMode::Read) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(NativeHooksError::Audit),
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| NativeHooksError::Audit)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| NativeHooksError::Audit)
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
    let Some(value) = read_record(&path)? else {
        return Ok(false);
    };
    let same = value["target"]["agent"].as_str() == Some(change.agent.name())
        && value["transition"]["requested"].as_bool() == Some(change.enabled)
        && value["transition"]["after"].as_bool() == Some(change.enabled)
        && value["correlationId"].as_str() == Some(change.request_id.as_str());
    if same {
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
    let Some(value) = read_record(&path)? else {
        return Ok(None);
    };
    if value["transition"]["requested"].as_bool() != Some(change.enabled) {
        return Err(NativeHooksError::Audit);
    }
    value["transition"]["before"]
        .as_bool()
        .map(Some)
        .ok_or(NativeHooksError::Audit)
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
        let id = record_name(
            evidence.agent,
            &evidence.principal_id,
            &evidence.request_id,
            evidence.phase,
        );
        let value = json!({
            "recordId": id,
            "kind": "native_hooks",
            "target": {"agent": evidence.agent.name()},
            "transition": {
                "before": evidence.before,
                "requested": evidence.requested,
                "after": evidence.after,
                "phase": evidence.phase.name(),
            },
            "cause": "caller_requested",
            "initiator": {
                "kind": evidence.caller_kind,
                "principalId": evidence.principal_id,
                "surfaceId": evidence.surface_id,
                "organizationId": evidence.organization_id,
            },
            "correlationId": evidence.request_id,
            "observedAtMs": self.clock.unix_milliseconds(),
        });
        let destination = self.directory.join(format!("{id}.json"));
        if agrees(&destination, &value)? {
            return sync_directory(&self.directory).map_err(|_| NativeHooksError::Audit);
        }
        let mut file =
            PrivateTempFile::new_in(&self.directory).map_err(|_| NativeHooksError::Audit)?;
        serde_json::to_writer(file.as_file_mut(), &value).map_err(|_| NativeHooksError::Audit)?;
        file.as_file_mut()
            .write_all(b"\n")
            .and_then(|_| file.as_file().sync_all())
            .map_err(|_| NativeHooksError::Audit)?;
        if let Err(error) = file.publish(&destination) {
            if error.kind() != ErrorKind::AlreadyExists || !agrees(&destination, &value)? {
                return Err(NativeHooksError::Audit);
            }
        }
        sync_directory(&self.directory).map_err(|_| NativeHooksError::Audit)
    }
}

fn agrees(destination: &Path, expected: &Value) -> Result<bool, NativeHooksError> {
    let Some(stored) = read_record(destination)? else {
        return Ok(false);
    };
    // The observation time is assigned when the record is first written. A
    // retry builds a new time and still matches that record.
    Ok(without_observation(&stored) == without_observation(expected))
}

fn without_observation(value: &Value) -> Value {
    let mut copy = value.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove("observedAtMs");
    }
    copy
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
        let value: Value = serde_json::from_str(&requested).unwrap();
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
        let value: Value = serde_json::from_str(&requested).unwrap();
        assert_eq!(value["observedAtMs"], 10);
        assert_eq!(value["transition"]["before"], false);
        assert_eq!(value["transition"]["requested"], true);
        assert!(value["transition"]["after"].is_null());
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
        let value: Value = serde_json::from_str(&recorded).unwrap();
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
    }
    #[cfg(unix)]
    impl NativeHooksAudit for LockOnOutcome {
        fn record(&self, evidence: &NativeHooksEvidence) -> Result<(), NativeHooksError> {
            use std::os::unix::fs::PermissionsExt;
            if evidence.phase == NativeHooksPhase::Outcome {
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
    async fn a_failed_revert_leaves_the_flag_with_the_published_file() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let audit = Arc::new(LockOnOutcome {
            directory: directory.clone(),
        });
        let preferences = NativeHooksPreferences::open_with(directory.clone(), audit).unwrap();
        let _restore = RestoreMode(directory.clone());
        let error = preferences
            .set(change(NativeHooksAgent::Codex, true, "request-7"))
            .await
            .unwrap_err();
        assert_eq!(error, NativeHooksError::Audit);
        assert!(preferences.selection().codex());
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        let stored = std::fs::read_to_string(directory.join("native-hooks.json")).unwrap();
        assert!(selection_from_stored(stored.as_bytes()).codex());
    }
}
