//! `machine.rs` reads a machine's name, processors, memory and disk
//! ([`SystemMachine`]); `sightings.rs` keeps each worker's last sighting as
//! a file of its own ([`FileSightings`]).
mod machine;
mod sightings;

pub(crate) use machine::SystemMachine;
pub(crate) use sightings::FileSightings;

use crate::devices::application::WallClock;

/// The process's wall clock.
pub(crate) struct SystemWallClock;

impl WallClock for SystemWallClock {
    fn now_ms(&self) -> u64 {
        nessa_sdk::infrastructure::clock::process_unix_ms()
    }
}
