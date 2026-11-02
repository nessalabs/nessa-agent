//! [`SightingStore`] as one private file per worker,
//! `<directory>/<key in hex>.json`, each replaced whole.
use crate::devices::application::{Sighting, SightingStore};
use nessa_local_storage::{create_directory, open, sync_directory, OpenMode, PrivateTempFile};
use nessa_protocol::lease::MachineReport;
use nessa_sdk::domain::agent_execution::leases::PeerEnvironment;
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    path::PathBuf,
};

/// The most a sighting's file may hold: a report's text is bounded, so a
/// longer file is not one this wrote.
const MAX_FILE_BYTES: u64 = 4096;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Saved {
    machine: MachineReport,
    seen_ms: u64,
}

/// `<conversations>/workers`, one file per worker.
pub(crate) struct FileSightings {
    directory: PathBuf,
}

impl FileSightings {
    pub(crate) fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    /// `<directory>/<key in hex>.json`.
    fn path(&self, peer: PeerEnvironment) -> PathBuf {
        let name = peer.name();
        let hex = name.strip_prefix(PeerEnvironment::PREFIX).unwrap_or(&name);
        self.directory.join(format!("{hex}.json"))
    }

    fn read(&self, path: &std::path::Path) -> io::Result<Sighting> {
        let mut bytes = Vec::new();
        open(path, OpenMode::Read)?
            .take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)?;
        let saved: Saved = serde_json::from_slice(&bytes)
            .ok()
            .filter(|saved: &Saved| saved.machine.is_bounded())
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?;
        Ok(Sighting {
            machine: saved.machine,
            seen_ms: saved.seen_ms,
        })
    }
}

impl SightingStore for FileSightings {
    fn load(&self) -> io::Result<Vec<(PeerEnvironment, Sighting)>> {
        let entries = match std::fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut sightings = Vec::new();
        for entry in entries {
            let path = entry?.path();
            let peer = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix(".json"))
                .and_then(|hex| {
                    PeerEnvironment::parse(&format!("{}{hex}", PeerEnvironment::PREFIX)).ok()
                });
            // A temporary file or anything else beside them is not a sighting.
            let Some(peer) = peer else { continue };
            match self.read(&path) {
                Ok(sighting) => sightings.push((peer, sighting)),
                Err(error) => tracing::warn!(?error, ?path, "a worker sighting is unreadable"),
            }
        }
        Ok(sightings)
    }

    fn save(&self, peer: PeerEnvironment, sighting: &Sighting) -> io::Result<()> {
        create_directory(&self.directory)?;
        let bytes = serde_json::to_vec(&Saved {
            machine: sighting.machine.clone(),
            seen_ms: sighting.seen_ms,
        })
        .map_err(io::Error::other)?;
        let mut file = PrivateTempFile::new_in(&self.directory)?;
        file.as_file_mut().write_all(&bytes)?;
        file.as_file().sync_all()?;
        file.persist(&self.path(peer))?;
        sync_directory(&self.directory)
    }

    fn remove(&self, peer: PeerEnvironment) -> io::Result<()> {
        match std::fs::remove_file(self.path(peer)) {
            Ok(()) => sync_directory(&self.directory),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sighting(seen_ms: u64) -> Sighting {
        Sighting {
            machine: MachineReport {
                hostname: "build-box".into(),
                os: "linux".into(),
                os_version: "24.04".into(),
                arch: "x86_64".into(),
                cpus: 8,
                cpu_permille: 610,
                memory_total_bytes: 32,
                memory_used_bytes: 8,
                disk_total_bytes: 100,
                disk_used_bytes: 40,
                disks: vec![nessa_protocol::lease::MachineDisk {
                    mount: "/".into(),
                    total_bytes: 100,
                    used_bytes: 40,
                }],
            },
            seen_ms,
        }
    }

    #[test]
    fn a_saved_sighting_is_read_back_and_replaced_whole() {
        let directory = tempfile::tempdir().unwrap();
        let store = FileSightings::new(directory.path().join("workers"));
        assert!(store.load().unwrap().is_empty());
        let peer = PeerEnvironment::new([7; PeerEnvironment::KEY_BYTES]);
        store.save(peer, &sighting(1)).unwrap();
        store.save(peer, &sighting(2)).unwrap();
        assert_eq!(store.load().unwrap(), vec![(peer, sighting(2))]);
        store.remove(peer).unwrap();
        store.remove(peer).unwrap();
        assert!(store.load().unwrap().is_empty());
    }

    #[test]
    fn a_file_that_is_not_a_sighting_is_left_out() {
        let directory = tempfile::tempdir().unwrap();
        let workers = directory.path().join("workers");
        std::fs::create_dir_all(&workers).unwrap();
        std::fs::write(workers.join(format!("{}.json", "ab".repeat(32))), b"{}").unwrap();
        std::fs::write(workers.join("notes.txt"), b"hello").unwrap();
        assert!(FileSightings::new(workers).load().unwrap().is_empty());
    }
}
