//! The host's one local-file repair (ADR 221): a settings file, or its folder,
//! that only the person can write but others can read is made private, and the
//! change is recorded before and after it is made.
//!
//! The record is the audit evidence. If the record of what is about to change
//! cannot be written, nothing is changed and the read stays refused.

use nessa_local_storage::{find_shared_read, SharedReadKind};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

/// Folder, beneath the config root, holding one record per repair step.
pub(crate) const RECORDS: &str = "local-storage-repairs";

/// Makes shared-read settings objects private, recording each change.
pub(crate) struct SharedReadRepair {
    records: PathBuf,
}

impl SharedReadRepair {
    /// Records go in `records`, which is created private when first needed.
    pub(crate) fn recording_in(records: PathBuf) -> Self {
        Self { records }
    }

    /// Tighten `path` if it is shared only for reading. `Ok(false)` means it
    /// was already private. An error means it was not changed: either it is not
    /// repairable, or the record of the change could not be written.
    pub(crate) fn repair(&self, path: &Path, kind: SharedReadKind) -> io::Result<bool> {
        let Some(candidate) = find_shared_read(path, kind)? else {
            return Ok(false);
        };
        let repair = record_id()?;
        let (before, after) = (candidate.mode_before(), candidate.mode_after());
        self.record(&repair, "intent", path, kind, (before, after), None)?;
        let result = candidate.tighten();
        let outcome = match &result {
            Ok(()) => "tightened".to_owned(),
            Err(error) => format!("failed: {error}"),
        };
        let recorded = self.record(
            &repair,
            "outcome",
            path,
            kind,
            (before, after),
            Some(&outcome),
        );
        result?;
        recorded?;
        eprintln!(
            "[nessa] made {} private (it was readable by other users)",
            path.display()
        );
        Ok(true)
    }

    fn record(
        &self,
        repair: &str,
        stage: &str,
        path: &Path,
        kind: SharedReadKind,
        (before, after): (u32, u32),
        outcome: Option<&str>,
    ) -> io::Result<()> {
        let record = serde_json::json!({
            "repair": repair,
            "stage": stage,
            "target": path.to_string_lossy(),
            "kind": match kind {
                SharedReadKind::File => "file",
                SharedReadKind::Directory => "directory",
            },
            "modeBefore": format!("{before:04o}"),
            "modeAfter": format!("{after:04o}"),
            "cause": "shared_read",
            "initiator": "desktop_host",
            "outcome": outcome,
        });
        self.write(repair, stage, record)
    }

    fn write(&self, repair: &str, stage: &str, record: serde_json::Value) -> io::Result<()> {
        nessa_local_storage::create_directory(&self.records)?;
        let mut file = nessa_local_storage::PrivateTempFile::new_in(&self.records)?;
        serde_json::to_writer(file.as_file_mut(), &record)?;
        file.as_file_mut().write_all(b"\n")?;
        file.as_file().sync_all()?;
        file.persist(&self.records.join(format!("{repair}-{stage}.json")))?;
        nessa_local_storage::sync_directory(&self.records)
    }
}

fn record_id() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn records(directory: &Path) -> Vec<serde_json::Value> {
        let mut records: Vec<serde_json::Value> = std::fs::read_dir(directory)
            .map(|entries| {
                entries
                    .map(|entry| {
                        serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap())
                            .unwrap()
                    })
                    .collect()
            })
            .unwrap_or_default();
        records.sort_by_key(|record| record["stage"].as_str().unwrap() == "outcome");
        records
    }

    #[test]
    fn a_shared_read_file_is_tightened_between_two_records() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.json");
        std::fs::write(&path, b"{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let repair = SharedReadRepair::recording_in(root.path().join(RECORDS));

        assert!(repair.repair(&path, SharedReadKind::File).unwrap());
        assert_eq!(mode_of(&path), 0o600);
        let written = records(&root.path().join(RECORDS));
        assert_eq!(written.len(), 2);
        for (record, stage, outcome) in [
            (&written[0], "intent", serde_json::Value::Null),
            (&written[1], "outcome", serde_json::json!("tightened")),
        ] {
            assert_eq!(record["stage"], stage);
            assert_eq!(record["target"], path.to_string_lossy().as_ref());
            assert_eq!(record["kind"], "file");
            assert_eq!(record["modeBefore"], "0644");
            assert_eq!(record["modeAfter"], "0600");
            assert_eq!(record["cause"], "shared_read");
            assert_eq!(record["initiator"], "desktop_host");
            assert_eq!(record["outcome"], outcome);
        }
        assert_eq!(written[0]["repair"], written[1]["repair"]);

        assert!(!repair.repair(&path, SharedReadKind::File).unwrap());
        assert_eq!(records(&root.path().join(RECORDS)).len(), 2);
    }

    #[test]
    fn nothing_changes_when_the_intent_cannot_be_recorded() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.json");
        std::fs::write(&path, b"{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        // A shared folder where the records belong: the record cannot be
        // written privately, so the repair must not happen.
        let records = root.path().join(RECORDS);
        std::fs::create_dir(&records).unwrap();
        std::fs::set_permissions(&records, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(SharedReadRepair::recording_in(records)
            .repair(&path, SharedReadKind::File)
            .is_err());
        assert_eq!(mode_of(&path), 0o644);
    }

    #[test]
    fn a_file_others_can_write_is_refused_and_left_alone() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.json");
        std::fs::write(&path, b"{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o664)).unwrap();
        let records = root.path().join(RECORDS);

        assert!(SharedReadRepair::recording_in(records.clone())
            .repair(&path, SharedReadKind::File)
            .is_err());
        assert_eq!(mode_of(&path), 0o664);
        assert!(!records.exists());
    }
}
