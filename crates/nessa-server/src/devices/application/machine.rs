//! What this machine is and how busy it is, as `env serve` reports it to its
//! gateway ([`nessa_protocol::lease::FromEnvironment::Machine`]) and as a
//! gateway reads its own for `devices.list`. The figures are shown to a
//! person and decide nothing.
use nessa_protocol::lease::MachineReport;
use std::{sync::Arc, time::Duration};

/// The longest a read may take before it is given up: a lease writer waits
/// on it, and must write again well inside the gateway's silence limit.
pub(crate) const MACHINE_READ_LIMIT: Duration = Duration::from_secs(2);

/// Reads this machine's [`MachineReport`]. Blocking: a caller on the
/// runtime reads it on a blocking thread.
pub(crate) trait MachineProbe: Send + Sync {
    /// The machine now, or `None` when it cannot be read; serving goes on
    /// without a report then.
    fn read(&self) -> Option<MachineReport>;
}

/// `probe`'s report, read on a blocking thread and given up after
/// [`MACHINE_READ_LIMIT`]; `None` too for one out of its bounds.
pub(crate) async fn read_machine(probe: &Arc<dyn MachineProbe>) -> Option<MachineReport> {
    let probe = probe.clone();
    tokio::time::timeout(
        MACHINE_READ_LIMIT,
        tokio::task::spawn_blocking(move || probe.read()),
    )
    .await
    .ok()?
    .ok()
    .flatten()
    .filter(MachineReport::is_bounded)
}
