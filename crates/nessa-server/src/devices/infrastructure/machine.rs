//! [`MachineProbe`] over `sysinfo`: the host's name, system, processors,
//! memory, and the disk holding a given directory.
use crate::devices::application::MachineProbe;
use nessa_protocol::lease::MachineReport;
use std::{
    path::PathBuf,
    sync::{Mutex, PoisonError},
};
use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, RefreshKind, System};

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
        let mut held = self.system.lock().unwrap_or_else(PoisonError::into_inner);
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

/// The size and use of the disk `path` is on: the mounted disk whose mount
/// point is the longest prefix of it; nothing known is `(0, 0)`.
fn disk_of(path: &std::path::Path) -> (u64, u64) {
    let path = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let disks = Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter(|disk| path.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().as_os_str().len())
        .map(|disk| {
            let total = disk.total_space();
            (total, total.saturating_sub(disk.available_space()))
        })
        .unwrap_or((0, 0))
}
