//! [`MachineProbe`] over `sysinfo`: the host's name, system, processors,
//! memory, and its local disks.
use crate::devices::application::MachineProbe;
use nessa_protocol::lease::{MachineDisk, MachineReport, MAX_MACHINE_DISKS, MAX_MACHINE_FIGURE};
use std::{
    path::PathBuf,
    sync::{Mutex, PoisonError, TryLockError},
};
use sysinfo::{CpuRefreshKind, DiskRefreshKind, Disks, MemoryRefreshKind, RefreshKind, System};

/// This machine. With no local disk found, its disk is the one `data` is on.
pub(crate) struct SystemMachine {
    data: PathBuf,
    /// Kept between reads: processor use is measured between two of them.
    system: Mutex<Option<System>>,
    /// The last report read, answered again while another read runs.
    last: Mutex<Option<MachineReport>>,
}

impl SystemMachine {
    pub(crate) fn new(data: PathBuf) -> Self {
        Self {
            data,
            system: Mutex::new(None),
            last: Mutex::new(None),
        }
    }
}

impl MachineProbe for SystemMachine {
    fn read(&self) -> Option<MachineReport> {
        // A read still running (one given up on, say) holds it: this one
        // answers the last report, rather than wait behind it on another
        // thread.
        let mut held = match self.system.try_lock() {
            Ok(held) => held,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                return self
                    .last
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
            }
        };
        let system = match held.as_mut() {
            Some(system) => {
                system.refresh_cpu_usage();
                system.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
                system
            }
            None => {
                // The first read has nothing to measure processor use
                // against: it measures over the shortest interval sysinfo
                // allows.
                let mut system = System::new_with_specifics(
                    RefreshKind::nothing()
                        .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
                        .with_memory(MemoryRefreshKind::nothing().with_ram()),
                );
                std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
                system.refresh_cpu_usage();
                held.insert(system)
            }
        };
        let cpus = u32::try_from(system.cpus().len()).unwrap_or(u32::MAX);
        let usage = system.global_cpu_usage();
        let cpu_permille = if usage.is_finite() {
            (usage.clamp(0.0, 100.0) * 10.0).round() as u32
        } else {
            0
        };
        let memory_total_bytes = system.total_memory();
        let memory_used_bytes = system.used_memory().min(memory_total_bytes);
        let mut disks = local_disks();
        if disks.is_empty() {
            let (total_bytes, used_bytes) = disk_of(&self.data);
            if total_bytes > 0 {
                disks.push(MachineDisk {
                    mount: MachineReport::bounded(&self.data.to_string_lossy()),
                    total_bytes,
                    used_bytes,
                });
            }
        }
        let sum = |figure: fn(&MachineDisk) -> u64| {
            disks
                .iter()
                .map(figure)
                .fold(0u64, u64::saturating_add)
                .min(MAX_MACHINE_FIGURE)
        };
        let disk_total_bytes = sum(|disk| disk.total_bytes);
        let disk_used_bytes = sum(|disk| disk.used_bytes).min(disk_total_bytes);
        let report = MachineReport {
            hostname: MachineReport::bounded(&System::host_name().unwrap_or_default()),
            os: MachineReport::bounded(std::env::consts::OS),
            os_version: MachineReport::bounded(&System::os_version().unwrap_or_default()),
            arch: MachineReport::bounded(std::env::consts::ARCH),
            cpus,
            cpu_permille,
            memory_total_bytes,
            memory_used_bytes,
            disk_total_bytes,
            disk_used_bytes,
            disks,
        };
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) = Some(report.clone());
        Some(report)
    }
}

/// File systems kept on a disk of this machine. A network share, a memory
/// file system or a packed image is not one, and is never asked its size: a
/// share that does not answer would hold the read.
const LOCAL_FILE_SYSTEMS: &[&str] = &[
    "apfs", "hfs", "ext2", "ext3", "ext4", "xfs", "btrfs", "zfs", "f2fs", "bcachefs", "ntfs",
    "refs", "exfat",
];

/// Where a volume of the system's own is mounted, beside the one disk it
/// shares: macOS mounts its data, virtual memory and boot volumes there.
const SYSTEM_VOLUMES: &[&str] = &["/System/Volumes/", "/private/var/vm", "/boot"];

/// Each local disk, once however many volumes share it, the root first.
/// Listed without asking any its size, then each kept one is asked alone.
fn local_disks() -> Vec<MachineDisk> {
    let listed = Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing());
    let mut seen = std::collections::BTreeSet::new();
    let mut kept: Vec<_> = listed
        .list()
        .iter()
        .filter(|disk| {
            let system = disk.file_system().to_string_lossy().to_ascii_lowercase();
            let mount = disk.mount_point().to_string_lossy();
            LOCAL_FILE_SYSTEMS.contains(&system.as_str())
                && !SYSTEM_VOLUMES
                    .iter()
                    .any(|volume| mount.starts_with(volume.trim_end_matches('/')))
        })
        .filter_map(|disk| {
            let (total_bytes, used_bytes) = disk_of(disk.mount_point());
            // A device mounted twice is one disk; so are the volumes of one
            // APFS container, which each answer the container's size and
            // what it has free.
            let shared = if disk.file_system().eq_ignore_ascii_case("apfs") {
                format!("apfs {total_bytes} {}", total_bytes - used_bytes)
            } else {
                format!("device {}", disk.name().to_string_lossy())
            };
            (total_bytes > 0 && seen.insert(shared)).then(|| MachineDisk {
                mount: MachineReport::bounded(&disk.mount_point().to_string_lossy()),
                total_bytes: total_bytes.min(MAX_MACHINE_FIGURE),
                used_bytes: used_bytes.min(total_bytes.min(MAX_MACHINE_FIGURE)),
            })
        })
        .collect();
    kept.sort_by(|left, right| {
        (left.mount != "/", &left.mount).cmp(&(right.mount != "/", &right.mount))
    });
    kept.truncate(MAX_MACHINE_DISKS);
    kept
}

/// The size and use of the disk `path` is on, asked of that one file
/// system (`statvfs`, `GetDiskFreeSpaceEx`, once for its size and once for
/// what is free): never a walk of every mount,
/// where a network share that does not answer would hold the read.
/// Nothing known is `(0, 0)`.
fn disk_of(path: &std::path::Path) -> (u64, u64) {
    match (fs2::total_space(path), fs2::available_space(path)) {
        (Ok(total), Ok(available)) => (total, total.saturating_sub(available)),
        _ => (0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_at_once_each_answer_the_machine() {
        let directory = tempfile::tempdir().unwrap();
        let machine = std::sync::Arc::new(SystemMachine::new(directory.path().to_path_buf()));
        // The first read holds the probe while it measures processor use;
        // one asked meanwhile answers the last report, once there is one.
        let first = machine.read().unwrap();
        assert!(first.is_bounded() && first.memory_total_bytes > 0);
        let reads: Vec<_> = (0..4)
            .map(|_| {
                let machine = machine.clone();
                std::thread::spawn(move || machine.read())
            })
            .collect();
        for read in reads {
            assert!(read
                .join()
                .unwrap()
                .is_some_and(|report| report.is_bounded()));
        }
    }
}
