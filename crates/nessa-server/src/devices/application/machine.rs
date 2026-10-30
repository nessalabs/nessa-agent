//! What this machine is and how busy it is, as `env serve` reports it to its
//! gateway ([`nessa_protocol::lease::FromEnvironment::Machine`]) and as a
//! gateway reads its own for `devices.list`. The figures are shown to a
//! person and decide nothing.
use nessa_protocol::lease::MachineReport;

/// Reads this machine's [`MachineReport`]. Blocking: a caller on the
/// runtime reads it on a blocking thread.
pub(crate) trait MachineProbe: Send + Sync {
    /// The machine now, or `None` when it cannot be read; serving goes on
    /// without a report then.
    fn read(&self) -> Option<MachineReport>;
}
