use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs::File,
    io::{self, Read, Write},
    path::Path,
    sync::Arc,
};

use nessa_auth::application::ports::Clock;
use nessa_local_storage::{
    create_private_directory_tree_beneath, is_private_temporary_name, OpenMode, PrivateDirectory,
    PrivateFileType,
};
use serde::{de::DeserializeOwned, Serialize};
use uuid::Uuid;

use super::record::{StoredOutcome, StoredPreparation, StoredSettlement};
use crate::agent_install::{
    application::{
        InstallDeliveryFailure, InstallDeliveryFailureStage, InstallationDelivery,
        InstallationDeliverySession, PendingInstallationDelivery, PreparedInstallation,
    },
    domain::{PublicationOutcome, PublicationPreparation, PublicationSettlement},
};

const LOCK_NAME: &str = "delivery.lock";
const MAX_RECORD_BYTES: usize = 64 * 1024;

pub struct DurableInstallationDelivery {
    directory: PrivateDirectory,
    original_lock: File,
    clock: Arc<dyn Clock>,
}

impl DurableInstallationDelivery {
    pub fn new(
        root: &Path,
        directory: &Path,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, InstallDeliveryFailure> {
        create_private_directory_tree_beneath(root, directory)
            .map_err(|error| failure(InstallDeliveryFailureStage::Initialize, error))?;
        let directory = PrivateDirectory::open_beneath(root, directory)
            .map_err(|error| failure(InstallDeliveryFailureStage::Initialize, error))?;
        let original_lock = directory
            .open_file(OsStr::new(LOCK_NAME), OpenMode::OpenOrCreate)
            .map_err(|error| failure(InstallDeliveryFailureStage::Initialize, error))?;
        original_lock
            .sync_all()
            .map_err(|error| failure(InstallDeliveryFailureStage::Initialize, error))?;
        ensure_named_lock(
            &directory,
            &original_lock,
            InstallDeliveryFailureStage::Initialize,
        )?;
        directory
            .sync()
            .map_err(|error| failure(InstallDeliveryFailureStage::Initialize, error))?;
        Ok(Self {
            directory,
            original_lock,
            clock,
        })
    }

    fn verify(&self, current_lock: &File) -> Result<(), InstallDeliveryFailure> {
        self.directory
            .verify_binding()
            .map_err(|error| failure(InstallDeliveryFailureStage::ReadState, error))?;
        ensure_named_lock(
            &self.directory,
            &self.original_lock,
            InstallDeliveryFailureStage::ReadState,
        )?;
        ensure_named_lock(
            &self.directory,
            current_lock,
            InstallDeliveryFailureStage::ReadState,
        )
    }

    fn open_session(
        &self,
        account_id: &str,
    ) -> Result<DeliverySession<'_>, InstallDeliveryFailure> {
        let current_lock = self
            .directory
            .open_file(OsStr::new(LOCK_NAME), OpenMode::ReadWrite)
            .map_err(|error| failure(InstallDeliveryFailureStage::AcquireLock, error))?;
        current_lock
            .lock()
            .map_err(|error| failure(InstallDeliveryFailureStage::AcquireLock, error))?;
        self.verify(&current_lock)?;
        Ok(DeliverySession {
            delivery: self,
            account_id: account_id.to_owned(),
            current_lock,
        })
    }
}

impl InstallationDelivery for DurableInstallationDelivery {
    fn session(
        &self,
        account_id: &str,
    ) -> Result<Box<dyn InstallationDeliverySession + '_>, InstallDeliveryFailure> {
        Ok(Box::new(self.open_session(account_id)?))
    }
}

struct DeliverySession<'delivery> {
    delivery: &'delivery DurableInstallationDelivery,
    account_id: String,
    current_lock: File,
}

impl InstallationDeliverySession for DeliverySession<'_> {
    fn pending(&mut self) -> Result<Option<PendingInstallationDelivery>, InstallDeliveryFailure> {
        self.delivery.verify(&self.current_lock)?;
        let pending = scan(&self.delivery.directory, &self.account_id)?;
        self.delivery.verify(&self.current_lock)?;
        Ok(pending)
    }

    fn prepare(
        &mut self,
        preparation: PublicationPreparation,
    ) -> Result<PreparedInstallation, InstallDeliveryFailure> {
        self.prepare_with_acknowledger(preparation, || Ok(()))
    }

    fn retain_outcome(
        &mut self,
        prepared: &PreparedInstallation,
        outcome: &PublicationOutcome,
    ) -> Result<(), InstallDeliveryFailure> {
        if prepared.preparation().verified().request().account_id() != self.account_id {
            return Err(failure(
                InstallDeliveryFailureStage::RetainOutcome,
                "prepared publication belongs to another account",
            ));
        }
        if &outcome.preparation() != prepared.preparation() {
            return Err(failure(
                InstallDeliveryFailureStage::RetainOutcome,
                "publication outcome disagrees with its preparation",
            ));
        }
        match scan(&self.delivery.directory, &self.account_id)? {
            Some(PendingInstallationDelivery::Prepared(current)) if current == *prepared => {}
            Some(_) => {
                return Err(failure(
                    InstallDeliveryFailureStage::RetainOutcome,
                    "publication outcome does not follow the durable preparation",
                ));
            }
            None => {
                return Err(failure(
                    InstallDeliveryFailureStage::RetainOutcome,
                    "publication outcome has no durable preparation",
                ));
            }
        }
        let stored = StoredOutcome::new(
            prepared.record_id().to_owned(),
            self.delivery.clock.unix_milliseconds(),
            outcome,
        );
        publish(
            &self.delivery.directory,
            &format!("{}.outcome.json", prepared.record_id()),
            &stored,
            InstallDeliveryFailureStage::RetainOutcome,
        )?;
        self.delivery.verify(&self.current_lock)
    }

    fn settle(
        &mut self,
        prepared: &PreparedInstallation,
        settlement: &PublicationSettlement,
    ) -> Result<(), InstallDeliveryFailure> {
        if prepared.preparation().verified().request().account_id() != self.account_id {
            return Err(failure(
                InstallDeliveryFailureStage::Settle,
                "prepared publication belongs to another account",
            ));
        }
        if settlement.outcome().preparation() != *prepared.preparation() {
            return Err(failure(
                InstallDeliveryFailureStage::Settle,
                "publication settlement disagrees with its preparation",
            ));
        }
        match scan(&self.delivery.directory, &self.account_id)? {
            Some(PendingInstallationDelivery::Outcome {
                prepared: current,
                outcome,
            }) if current == *prepared && &outcome == settlement.outcome() => {}
            Some(_) => {
                return Err(failure(
                    InstallDeliveryFailureStage::Settle,
                    "publication settlement does not follow the retained outcome",
                ));
            }
            None => {
                return Err(failure(
                    InstallDeliveryFailureStage::Settle,
                    "publication settlement has no retained outcome",
                ));
            }
        }
        let stored = StoredSettlement::new(
            prepared.record_id().to_owned(),
            self.delivery.clock.unix_milliseconds(),
            settlement,
        );
        publish(
            &self.delivery.directory,
            &format!("{}.settled.json", prepared.record_id()),
            &stored,
            InstallDeliveryFailureStage::Settle,
        )?;
        self.delivery.verify(&self.current_lock)
    }
}

impl DeliverySession<'_> {
    fn prepare_with_acknowledger(
        &mut self,
        preparation: PublicationPreparation,
        acknowledge: impl FnOnce() -> Result<(), InstallDeliveryFailure>,
    ) -> Result<PreparedInstallation, InstallDeliveryFailure> {
        if preparation.verified().request().account_id() != self.account_id {
            return Err(failure(
                InstallDeliveryFailureStage::Prepare,
                "publication preparation belongs to another account",
            ));
        }
        if scan(&self.delivery.directory, &self.account_id)?.is_some() {
            return Err(failure(
                InstallDeliveryFailureStage::Prepare,
                "an unresolved publication already blocks this account",
            ));
        }
        let record_id = Uuid::new_v4().to_string();
        let stored = StoredPreparation::new(
            record_id.clone(),
            self.delivery.clock.unix_milliseconds(),
            &preparation,
        );
        publish(
            &self.delivery.directory,
            &format!("{record_id}.prepared.json"),
            &stored,
            InstallDeliveryFailureStage::Prepare,
        )?;
        acknowledge()?;
        self.delivery.verify(&self.current_lock)?;
        Ok(PreparedInstallation::new(record_id, preparation))
    }
}

#[derive(Default)]
struct StoredHistory {
    preparation: Option<StoredPreparation>,
    outcome: Option<StoredOutcome>,
    settlement: Option<StoredSettlement>,
}

fn scan(
    directory: &PrivateDirectory,
    account_id: &str,
) -> Result<Option<PendingInstallationDelivery>, InstallDeliveryFailure> {
    let mut histories = BTreeMap::<String, StoredHistory>::new();
    let entries = directory
        .entries()
        .map_err(|error| failure(InstallDeliveryFailureStage::ReadState, error))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| failure(InstallDeliveryFailureStage::ReadState, error))?;
        if entry.name() == OsStr::new(LOCK_NAME) {
            if entry.file_type() != PrivateFileType::RegularFile {
                return Err(failure(
                    InstallDeliveryFailureStage::ReadState,
                    "the delivery lock is not a regular file",
                ));
            }
            continue;
        }
        if is_private_temporary_name(entry.name()) {
            if entry.file_type() != PrivateFileType::RegularFile {
                return Err(failure(
                    InstallDeliveryFailureStage::ReadState,
                    "a delivery reservation is not a regular file",
                ));
            }
            continue;
        }
        if entry.file_type() != PrivateFileType::RegularFile {
            return Err(failure(
                InstallDeliveryFailureStage::ReadState,
                "an unexpected delivery entry is not a regular file",
            ));
        }
        let name = entry.name().to_str().ok_or_else(|| {
            failure(
                InstallDeliveryFailureStage::ReadState,
                "delivery record name is not UTF-8",
            )
        })?;
        let (record_id, kind) = parse_name(name)?;
        let history = histories.entry(record_id.to_owned()).or_default();
        match kind {
            RecordKind::Preparation => {
                set_once(
                    &mut history.preparation,
                    read(directory, entry.name())?,
                    "duplicate preparation record",
                )?;
            }
            RecordKind::Outcome => {
                set_once(
                    &mut history.outcome,
                    read(directory, entry.name())?,
                    "duplicate outcome record",
                )?;
            }
            RecordKind::Settlement => {
                set_once(
                    &mut history.settlement,
                    read(directory, entry.name())?,
                    "duplicate settlement record",
                )?;
            }
        }
    }

    let mut pending = None;
    for (record_id, history) in histories {
        let preparation = history.preparation.ok_or_else(|| {
            failure(
                InstallDeliveryFailureStage::ReadState,
                format!("delivery history {record_id} has no preparation"),
            )
        })?;
        let prepared = preparation.restore()?;
        if prepared.record_id() != record_id {
            return Err(failure(
                InstallDeliveryFailureStage::ReadState,
                "preparation record id disagrees with its filename",
            ));
        }
        let outcome = history
            .outcome
            .as_ref()
            .map(|outcome| outcome.restore(&prepared))
            .transpose()?;
        if let Some(settlement) = history.settlement {
            let outcome = outcome.as_ref().ok_or_else(|| {
                failure(
                    InstallDeliveryFailureStage::ReadState,
                    "settlement has no retained outcome",
                )
            })?;
            settlement.restore(&prepared, outcome)?;
            continue;
        }
        if prepared.preparation().verified().request().account_id() != account_id {
            continue;
        }
        let current = match outcome {
            Some(outcome) => PendingInstallationDelivery::Outcome { prepared, outcome },
            None => PendingInstallationDelivery::Prepared(prepared),
        };
        if pending.replace(current).is_some() {
            return Err(failure(
                InstallDeliveryFailureStage::ReadState,
                "more than one unresolved publication exists for this account",
            ));
        }
    }
    Ok(pending)
}

#[derive(Clone, Copy)]
enum RecordKind {
    Preparation,
    Outcome,
    Settlement,
}

fn parse_name(name: &str) -> Result<(&str, RecordKind), InstallDeliveryFailure> {
    let (record_id, kind) = if let Some(record_id) = name.strip_suffix(".prepared.json") {
        (record_id, RecordKind::Preparation)
    } else if let Some(record_id) = name.strip_suffix(".outcome.json") {
        (record_id, RecordKind::Outcome)
    } else if let Some(record_id) = name.strip_suffix(".settled.json") {
        (record_id, RecordKind::Settlement)
    } else {
        return Err(failure(
            InstallDeliveryFailureStage::ReadState,
            format!("unexpected delivery entry {name}"),
        ));
    };
    let parsed = Uuid::parse_str(record_id).map_err(|_| {
        failure(
            InstallDeliveryFailureStage::ReadState,
            "delivery filename has an invalid record id",
        )
    })?;
    if parsed.to_string() != record_id {
        return Err(failure(
            InstallDeliveryFailureStage::ReadState,
            "delivery filename has a non-canonical record id",
        ));
    }
    Ok((record_id, kind))
}

fn set_once<T>(slot: &mut Option<T>, value: T, detail: &str) -> Result<(), InstallDeliveryFailure> {
    if slot.replace(value).is_some() {
        Err(failure(InstallDeliveryFailureStage::ReadState, detail))
    } else {
        Ok(())
    }
}

fn read<T: DeserializeOwned>(
    directory: &PrivateDirectory,
    name: &OsStr,
) -> Result<T, InstallDeliveryFailure> {
    let file = directory
        .open_file(name, OpenMode::Read)
        .map_err(|error| failure(InstallDeliveryFailureStage::ReadState, error))?;
    if !directory
        .named_file_is(name, &file)
        .map_err(|error| failure(InstallDeliveryFailureStage::ReadState, error))?
    {
        return Err(failure(
            InstallDeliveryFailureStage::ReadState,
            "delivery record changed while it was opened",
        ));
    }
    let mut encoded = Vec::new();
    file.take((MAX_RECORD_BYTES + 1) as u64)
        .read_to_end(&mut encoded)
        .map_err(|error| failure(InstallDeliveryFailureStage::ReadState, error))?;
    if encoded.len() > MAX_RECORD_BYTES {
        return Err(failure(
            InstallDeliveryFailureStage::ReadState,
            "delivery record exceeds its byte limit",
        ));
    }
    serde_json::from_slice(&encoded)
        .map_err(|error| failure(InstallDeliveryFailureStage::ReadState, error))
}

fn publish(
    directory: &PrivateDirectory,
    destination: &str,
    value: &impl Serialize,
    stage: InstallDeliveryFailureStage,
) -> Result<(), InstallDeliveryFailure> {
    let mut encoded = BoundedBuffer::new();
    serde_json::to_writer(&mut encoded, value).map_err(|error| failure(stage, error))?;
    encoded
        .write_all(b"\n")
        .map_err(|error| failure(stage, error))?;
    let mut reservation = directory
        .reserve_temp()
        .map_err(|error| failure(stage, error))?;
    reservation
        .as_file_mut()
        .write_all(encoded.as_slice())
        .map_err(|error| failure(stage, error))?;
    reservation
        .publish_new(OsStr::new(destination))
        .map_err(|error| failure(stage, error))?;
    Ok(())
}

struct BoundedBuffer {
    bytes: Box<[u8]>,
    len: usize,
}

impl BoundedBuffer {
    fn new() -> Self {
        Self {
            bytes: vec![0; MAX_RECORD_BYTES].into_boxed_slice(),
            len: 0,
        }
    }

    fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl Write for BoundedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let end = self
            .len
            .checked_add(bytes.len())
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| io::Error::other("delivery record exceeds its byte limit"))?;
        self.bytes[self.len..end].copy_from_slice(bytes);
        self.len = end;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn ensure_named_lock(
    directory: &PrivateDirectory,
    lock: &File,
    stage: InstallDeliveryFailureStage,
) -> Result<(), InstallDeliveryFailure> {
    match directory.named_file_is(OsStr::new(LOCK_NAME), lock) {
        Ok(true) => Ok(()),
        Ok(false) => Err(failure(stage, "delivery lock identity changed")),
        Err(error) => Err(failure(stage, error)),
    }
}

fn failure(
    stage: InstallDeliveryFailureStage,
    error: impl std::fmt::Display,
) -> InstallDeliveryFailure {
    InstallDeliveryFailure::new(stage, error.to_string())
}

#[cfg(test)]
#[path = "../../../../tests/agent_install/delivery.rs"]
mod tests;
