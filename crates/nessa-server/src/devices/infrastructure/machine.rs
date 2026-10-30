//! [`MachineProbe`] over `sysinfo`: the host's name, system, processors,
//! memory, and the disk holding a given directory.
use crate::devices::application::MachineProbe;
use nessa_protocol::lease::MachineReport;
use std::{
    path::PathBuf,
    sync::{Mutex, TryLockError},
};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

/// This machine, its disk being the one `data` is on.
pub(crate) struct SystemMachine {
    data: PathBuf,
    /// Kept between reads: processor use is measured between two of them.
    system: Mutex<Option<System>>,
}

impl SystemMachine {
    pub(crate) fn new(data: PathBuf) -> Self {
        Self {
            data,
            system: Mutex::new(None),
        }
    }
}

impl MachineProbe for SystemMachine {
    fn read(&self) -> Option<MachineReport> {
        // A read still running (one given up on, say) holds it: this one
        // has nothing, rather than wait behind it on another thread.
        let mut held = match self.system.try_lock() {
            Ok(held) => held,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return None,
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
        let (disk_total_bytes, disk_used_bytes) = disk_of(&self.data);
        Some(MachineReport {
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
        })
    }
}

/// The size and use of the disk `path` is on, asked of that one file
/// system (`statvfs`, `GetDiskFreeSpaceEx`): never a walk of every mount,
/// where a network share that does not answer would hold the read.
/// Nothing known is `(0, 0)`.
fn disk_of(path: &std::path::Path) -> (u64, u64) {
    match (fs2::total_space(path), fs2::available_space(path)) {
        (Ok(total), Ok(available)) => (total, total.saturating_sub(available)),
        _ => (0, 0),
    }
}
