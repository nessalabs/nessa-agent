//! The hub's devices (`devices.list`): the machine this gateway runs on and
//! the paired environments that work for it, with what each is and how busy.
//!
//! ```text
//! env serve ── Machine{report} (lease link) ──▶ HostLink keeps the latest
//! devices.list ──▶ Devices::list
//!   ──▶ MachineProbe (this machine, read off the runtime)
//!   ──▶ LiveWorkers (each paired environment's connection: load + report)
//!   ──▶ WorkerSightings (each worker's last report, saved: offline ones too)
//! a worker's connection ends ──▶ WorkerSightings::saw (its last report)
//! ```
//!
//! What a worker is doing now is read off its connection, never copied
//! here; only when it was last seen, and as what, is kept, so a worker that
//! is off still shows.
pub(crate) mod application;
pub(crate) mod infrastructure;
