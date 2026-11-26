//! Listing the hub's machine: [`Devices`] over a [`MachineProbe`].
use nessa_protocol::lease::MachineReport;
use std::{net::SocketAddr, sync::Arc, sync::OnceLock, time::Duration};

/// The longest a read may take before it is given up.
pub const MACHINE_READ_LIMIT: Duration = Duration::from_secs(2);

/// Reads this machine's [`MachineReport`]. Blocking: a caller on the
/// runtime reads it on a blocking thread.
pub trait MachineProbe: Send + Sync {
    /// The machine now, or `None` when it cannot be read.
    fn read(&self) -> Option<MachineReport>;
}

/// `probe`'s report, read on a blocking thread and given up after
/// [`MACHINE_READ_LIMIT`]; `None` too for one out of its bounds.
pub async fn read_machine(probe: &Arc<dyn MachineProbe>) -> Option<MachineReport> {
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

/// The hub as listed.
pub struct DeviceList {
    pub hub: Option<MachineReport>,
    /// Where the hub's native listener, which linked phones dial, is bound;
    /// `None` while native linking is off.
    pub listening: Option<SocketAddr>,
}

/// The hub's machine.
pub struct Devices {
    hub: Arc<dyn MachineProbe>,
    listening: OnceLock<SocketAddr>,
}

impl Devices {
    pub fn new(hub: Arc<dyn MachineProbe>) -> Self {
        Self {
            hub,
            listening: OnceLock::new(),
        }
    }

    /// The native listener is bound at `address`: said once, when it is.
    pub fn listening(&self, address: SocketAddr) {
        let _ = self.listening.set(address);
    }

    /// The hub as it reads now.
    pub async fn list(&self) -> DeviceList {
        DeviceList {
            hub: read_machine(&self.hub).await,
            listening: self.listening.get().copied(),
        }
    }
}
