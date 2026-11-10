//! Stage, accept, list, and open a quarantined restore without starting a gateway.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use nessa_local_storage::create_directory;

use super::backup_output::print;
use super::serving_restore::{
    install_serving_stores, merge_published_session, namespace_is_restore_root,
};
use crate::backup::application::{
    binding_from_pointer, confirm_accepted, deliver_historical_commands,
    list_accepted_conversations, open_accepted_conversation, stage_quarantine, AbsentSessionSource,
    AcceptReport, AcceptRequest, AcceptedBinding, AcceptedFetch, DeferredConversationOpen,
    DeferredOpenFailure, ExportClock, FetchProgress, InventoryCause, LeaseReceipts,
    QuarantineLayout, RestoreAction, RestoreFacts, StageRequest, VerificationLimits,
};
use crate::backup::domain::{
    credentials_imported_on_restore, encode_accepted_restore, paths_are_distinct, read_journal,
    RestoreFault,
};
use crate::backup::entrypoint::{parse_restore, RestoreCommand, RestoreUsageError, RESTORE_HELP};
use crate::backup::infrastructure::{
    read_verification_key, CedarInventoryPolicy, DirectoryMaterializer, DirectoryQuarantine,
    DirectoryVerificationSource, ExportFormat, FileRestoreAudit, KeyedInventory, OsPublicationIds,
    OsStagingIds, SqliteDeferredSource,
};
use crate::core::RunError;

const LATER_INVENTORY_LIMIT: usize = 4 * 1024 * 1024;

struct WallClock;

impl ExportClock for WallClock {
    fn unix_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    }
}

struct Quiet;

impl FetchProgress for Quiet {
    fn report(&mut self, _read: u64, _total: u64) {}
}

struct ArgReceipts(Vec<(String, String)>);

impl LeaseReceipts for ArgReceipts {
    fn receipt(&self, lease_id: &str) -> Option<String> {
        self.0
            .iter()
            .find(|(id, _)| id == lease_id)
            .map(|(_, text)| text.clone())
    }
}

fn person(action: RestoreAction) -> RestoreFacts<'static> {
    RestoreFacts {
        kind: "person",
        cause: InventoryCause::Operator,
        action,
    }
}

fn surface() -> RestoreFacts<'static> {
    RestoreFacts {
        kind: "surface",
        cause: InventoryCause::Operator,
        action: RestoreAction::Fetch,
    }
}

#[derive(serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum RestoreOutput {
    Quarantined {
        leases: Vec<String>,
        freshness: String,
    },
    Accepted {
        leases: Vec<String>,
    },
    Listed {
        conversations: Vec<String>,
    },
    Opened {
        size: u64,
        digest: String,
        already_present: bool,
    },
    Refused {
        reason: String,
    },
    Usage,
}

struct Roots {
    quarantine: PathBuf,
    live: PathBuf,
    audit: PathBuf,
}

fn roots(destination: &Path) -> Roots {
    Roots {
        quarantine: destination.join("quarantine"),
        live: destination.join("live"),
        audit: destination.join("audit"),
    }
}

/// Run one restore command. JSON goes to `output`. Exit 0 is a completed
/// stage, accept, list, or open. Exit 1 is a refusal. Exit 2 is usage.
pub fn run_backup_restore(arguments: &[std::ffi::OsString], output: &mut dyn Write) -> u8 {
    let command = match parse_restore(arguments) {
        Ok(command) => command,
        Err(RestoreUsageError) => return print(output, &RestoreOutput::Usage, 2),
    };
    if matches!(command, RestoreCommand::Help) {
        return if output.write_all(RESTORE_HELP.as_bytes()).is_ok() {
            0
        } else {
            1
        };
    }
    let policy = match CedarInventoryPolicy::new() {
        Ok(policy) => policy,
        Err(_) => {
            return print(
                output,
                &RestoreOutput::Refused {
                    reason: "unavailable".into(),
                },
                1,
            )
        }
    };
    match command {
        RestoreCommand::Help => 0,
        RestoreCommand::Stage {
            bundle,
            mac_key_file,
            destination,
            later_inventory,
            source,
        } => stage(
            output,
            &policy,
            &bundle,
            &mac_key_file,
            &destination,
            later_inventory.as_deref(),
            source.as_deref(),
        ),
        RestoreCommand::Accept {
            destination,
            namespace,
            receipts,
        } => accept(
            output,
            &policy,
            &destination,
            namespace.as_deref(),
            &receipts,
        ),
        RestoreCommand::List { destination } => list(output, &destination),
        RestoreCommand::Open {
            destination,
            conversation,
        } => open_one(output, &policy, &destination, &conversation),
    }
}

fn stage(
    output: &mut dyn Write,
    policy: &CedarInventoryPolicy,
    bundle: &Path,
    mac_key_file: &Path,
    destination: &Path,
    later_inventory: Option<&Path>,
    source: Option<&Path>,
) -> u8 {
    if let Some(source) = source {
        if !source.is_file() {
            return refused(output, RestoreFault::SourceMissing);
        }
    }
    let roots = roots(destination);
    if create_directory(destination).is_err() {
        return refused(output, RestoreFault::Unreadable);
    }
    let key = match read_verification_key(mac_key_file) {
        Ok(key) => key,
        Err(_) => return refused(output, RestoreFault::Verification),
    };
    let bundle = match DirectoryVerificationSource::open(bundle.to_path_buf()) {
        Ok(bundle) => bundle,
        Err(_) => return refused(output, RestoreFault::Verification),
    };
    let later = match later_inventory {
        Some(path) => match later_text(path) {
            Ok(text) => Some(text),
            Err(error) => return refused(output, error),
        },
        None => None,
    };
    let audit = match FileRestoreAudit::open(&roots.audit, WallClock) {
        Ok(audit) => audit,
        Err(_) => return refused(output, RestoreFault::Unreadable),
    };
    let report = stage_quarantine(
        &StageRequest {
            facts: person(RestoreAction::Stage),
            bundle: &bundle,
            format: &ExportFormat,
            verifier: &key,
            limits: VerificationLimits::default(),
            later_inventory: later.as_deref(),
            quarantine: &roots.quarantine,
            live: &roots.live,
        },
        policy,
        &audit,
        &DirectoryQuarantine,
        &DirectoryMaterializer,
        &KeyedInventory::new(key),
        &OsStagingIds,
    );
    let report = match report {
        Ok(report) => report,
        Err(error) => return refused(output, error),
    };
    if let Some(source) = source {
        let Some(text) = source.to_str() else {
            return refused(output, RestoreFault::Paths);
        };
        if fs::write(destination.join("source.txt"), format!("{text}\n")).is_err() {
            return refused(output, RestoreFault::Unreadable);
        }
    }
    if roots.live.exists() {
        return refused(output, RestoreFault::LiveOccupied);
    }
    print(
        output,
        &RestoreOutput::Quarantined {
            leases: report.leases,
            freshness: report.freshness.as_str().to_owned(),
        },
        0,
    )
}

fn accept(
    output: &mut dyn Write,
    policy: &CedarInventoryPolicy,
    destination: &Path,
    namespace: Option<&Path>,
    receipts: &[(String, String)],
) -> u8 {
    let roots = roots(destination);
    if let Some(namespace) = namespace {
        if paths_are_distinct(namespace, &roots.live).is_err() {
            return refused(output, RestoreFault::Paths);
        }
    }
    let audit = match FileRestoreAudit::open(&roots.audit, WallClock) {
        Ok(audit) => audit,
        Err(_) => return refused(output, RestoreFault::Unreadable),
    };
    let report = crate::backup::application::accept_quarantine(
        &AcceptRequest {
            facts: person(RestoreAction::Accept),
            quarantine: &roots.quarantine,
            live: &roots.live,
            receipts: &ArgReceipts(receipts.to_vec()),
        },
        policy,
        &audit,
        &DirectoryQuarantine,
    );
    let report = match report {
        Ok(report) => report,
        // The quarantine is spent once the rename has happened. A later
        // accept, including one whose pointer write failed, still records
        // the pointer when the live tree is already the authority.
        Err(error @ (RestoreFault::LiveOccupied | RestoreFault::Tamper)) => {
            match accepted_leases(&roots.live, &audit) {
                Ok(leases) => AcceptReport { leases },
                Err(_) => return refused(output, error),
            }
        }
        Err(error) => return refused(output, error),
    };
    if let Some(namespace) = namespace {
        let source = match recorded_source(destination) {
            Ok(source) => source,
            Err(error) => return refused(output, error),
        };
        let text = match encode_accepted_restore(&roots.live, &roots.audit, source.as_deref()) {
            Ok(text) => text,
            Err(error) => return refused(output, error),
        };
        if create_directory(&namespace.join("backup")).is_err()
            || fs::write(namespace.join("backup").join("accepted-restore.txt"), text).is_err()
        {
            return refused(output, RestoreFault::Unreadable);
        }
    }
    print(
        output,
        &RestoreOutput::Accepted {
            leases: report.leases,
        },
        0,
    )
}

fn list(output: &mut dyn Write, destination: &Path) -> u8 {
    let roots = roots(destination);
    let audit = match FileRestoreAudit::open(&roots.audit, WallClock) {
        Ok(audit) => audit,
        Err(_) => return refused(output, RestoreFault::Unreadable),
    };
    match list_accepted_conversations(&roots.live, &DirectoryQuarantine, &audit) {
        Ok(conversations) => print(output, &RestoreOutput::Listed { conversations }, 0),
        Err(error) => refused(output, error),
    }
}

fn open_one(
    output: &mut dyn Write,
    policy: &CedarInventoryPolicy,
    destination: &Path,
    conversation: &str,
) -> u8 {
    let roots = roots(destination);
    let audit = match FileRestoreAudit::open(&roots.audit, WallClock) {
        Ok(audit) => audit,
        Err(_) => return refused(output, RestoreFault::Unreadable),
    };
    let source = match recorded_source(destination) {
        Ok(source) => source,
        Err(error) => return refused(output, error),
    };
    let held: Box<dyn crate::backup::application::DeferredSessionSource> = match source {
        Some(path) if path.is_file() => Box::new(SqliteDeferredSource::new(path)),
        _ => Box::new(AbsentSessionSource),
    };
    let mut progress = CommandProgress {
        output,
        failed: false,
    };
    let opened = open_accepted_conversation(&mut AcceptedFetch {
        live: &roots.live,
        conversation_id: conversation,
        facts: person(RestoreAction::Fetch),
        policy,
        audit: &audit,
        layout: &DirectoryQuarantine,
        source: held.as_ref(),
        ids: &OsPublicationIds,
        progress: &mut progress,
    });
    if progress.failed {
        return 1;
    }
    match opened {
        Ok(report) => print(
            output,
            &RestoreOutput::Opened {
                size: report.size,
                digest: report.digest.hex(),
                already_present: report.already_present,
            },
            0,
        ),
        Err(error) => refused(output, error),
    }
}

struct CommandProgress<'a> {
    output: &'a mut dyn Write,
    failed: bool,
}

impl FetchProgress for CommandProgress<'_> {
    fn report(&mut self, read: u64, total: u64) {
        let line = format!("{{\"status\":\"progress\",\"read\":{read},\"total\":{total}}}\n");
        if self.output.write_all(line.as_bytes()).is_err() {
            self.failed = true;
        }
    }
}

fn accepted_leases(
    live: &Path,
    audit: &FileRestoreAudit<WallClock>,
) -> Result<Vec<String>, RestoreFault> {
    confirm_accepted(live, &DirectoryQuarantine, audit)?;
    let anchored = DirectoryQuarantine.anchor(live)?;
    let loaded = DirectoryQuarantine.load(&anchored)?;
    Ok(read_journal(&loaded.journal)?.leases)
}

fn later_text(path: &Path) -> Result<String, RestoreFault> {
    let file = File::open(path).map_err(|_| RestoreFault::Unreadable)?;
    let mut bytes = Vec::new();
    file.take(u64::try_from(LATER_INVENTORY_LIMIT).unwrap_or(u64::MAX) + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| RestoreFault::Unreadable)?;
    if bytes.len() > LATER_INVENTORY_LIMIT {
        return Err(RestoreFault::Unreadable);
    }
    String::from_utf8(bytes).map_err(|_| RestoreFault::Unreadable)
}

fn recorded_source(destination: &Path) -> Result<Option<PathBuf>, RestoreFault> {
    let path = destination.join("source.txt");
    if !path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(path).map_err(|_| RestoreFault::Unreadable)?;
    let line = text.trim_end_matches(['\n', '\r']);
    if line.is_empty() {
        return Ok(None);
    }
    let source = PathBuf::from(line);
    if !source.is_absolute() {
        return Err(RestoreFault::Paths);
    }
    Ok(Some(source))
}

fn refused(output: &mut dyn Write, error: RestoreFault) -> u8 {
    print(
        output,
        &RestoreOutput::Refused {
            reason: error.cause().to_owned(),
        },
        1,
    )
}

/// What startup found in `namespace/backup/accepted-restore.txt`.
///
/// Absent is an ordinary gateway. A pointer that is not an accepted live
/// restore, or one whose identity is not a fresh re-pair, refuses startup
/// before the gateway listens. Historical outbox names are not executed.
pub(crate) fn startup_authority(namespace: &Path) -> Result<Option<AcceptedBinding>, RunError> {
    if namespace_is_restore_root(namespace) {
        return Err(RunError::Restore(
            "this backup is still quarantined and has not been accepted as the authority".into(),
        ));
    }
    let pointer = namespace.join("backup").join("accepted-restore.txt");
    if !pointer.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(&pointer)
        .map_err(|_| RunError::Restore("the accepted restore pointer could not be read".into()))?;
    let binding = binding_from_pointer(&text)
        .map_err(|_| RunError::Restore("the accepted restore pointer is not usable".into()))?;
    if paths_are_distinct(namespace, &binding.live).is_err() {
        return Err(RunError::Restore(
            "the restore live path is this gateway's own data directory".into(),
        ));
    }
    let audit = FileRestoreAudit::open(&binding.audit, WallClock)
        .map_err(|_| RunError::Restore("the restore audit could not be opened".into()))?;
    crate::backup::application::confirm_accepted(&binding.live, &DirectoryQuarantine, &audit)
        .map_err(|error| {
            if error == RestoreFault::NotAccepted {
                RunError::Restore(
                    "this backup is still quarantined and has not been accepted as the authority"
                        .into(),
                )
            } else {
                RunError::Restore(
                    "this restored backup is not one this gateway can start from".into(),
                )
            }
        })?;
    let replay = fs::read(binding.live.join("replay.blocked")).unwrap_or_default();
    let commands = outbox_commands(&binding.live);
    let mut ran = false;
    deliver_historical_commands(&replay, &commands, &mut |_| ran = true).map_err(|_| {
        RunError::Restore("historical outbox bytes are not a command this gateway will run".into())
    })?;
    if ran {
        return Err(RunError::Restore(
            "historical outbox bytes are not a command this gateway will run".into(),
        ));
    }
    let identity = fs::read(binding.live.join("identity.txt")).unwrap_or_default();
    credentials_imported_on_restore(&identity, &[]).map_err(|_| {
        RunError::Restore("a restored gateway does not keep the old private key".into())
    })?;
    install_serving_stores(namespace, &binding.live).map_err(|error| {
        RunError::Restore(format!(
            "restored conversations were not installed: {error}"
        ))
    })?;
    Ok(Some(binding))
}

fn outbox_commands(live: &Path) -> Vec<String> {
    let mut names = Vec::new();
    for store in ["device-outbox", "environment-outbox"] {
        let Ok(entries) = fs::read_dir(live.join("records").join(store)) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.path().is_file() {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            names.push(format!("records/{store}/{name}"));
        }
    }
    names.sort();
    names
}

/// Opens a deferred conversation on the accepted restore before the
/// conversation service starts an agent.
pub(crate) struct GatewayDeferredOpen {
    namespace: PathBuf,
    binding: AcceptedBinding,
    policy: CedarInventoryPolicy,
}

impl GatewayDeferredOpen {
    pub(crate) fn new(
        namespace: PathBuf,
        binding: AcceptedBinding,
        policy: CedarInventoryPolicy,
    ) -> Self {
        Self {
            namespace,
            binding,
            policy,
        }
    }
}

impl DeferredConversationOpen for GatewayDeferredOpen {
    fn open(&self, conversation_id: &str) -> Result<(), DeferredOpenFailure> {
        let audit = FileRestoreAudit::open(&self.binding.audit, WallClock)
            .map_err(|_| DeferredOpenFailure::Unavailable)?;
        let held: Box<dyn crate::backup::application::DeferredSessionSource> =
            match &self.binding.source {
                Some(path) if path.is_file() => Box::new(SqliteDeferredSource::new(path)),
                _ => Box::new(AbsentSessionSource),
            };
        let mut progress = Quiet;
        match open_accepted_conversation(&mut AcceptedFetch {
            live: &self.binding.live,
            conversation_id,
            facts: surface(),
            policy: &self.policy,
            audit: &audit,
            layout: &DirectoryQuarantine,
            source: held.as_ref(),
            ids: &OsPublicationIds,
            progress: &mut progress,
        }) {
            Ok(_) => merge_published_session(&self.namespace, &self.binding.live, conversation_id)
                .map_err(|_| DeferredOpenFailure::Unavailable),
            Err(RestoreFault::NotDeferred) => Ok(()),
            Err(RestoreFault::SourceMissing) => Err(DeferredOpenFailure::SourceMissing),
            Err(RestoreFault::SourceChanged) => Err(DeferredOpenFailure::SourceChanged),
            Err(_) => Err(DeferredOpenFailure::Unavailable),
        }
    }
}
