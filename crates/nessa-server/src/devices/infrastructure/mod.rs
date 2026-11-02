//! `machine.rs` reads a machine's name, processors, memory and disk
//! ([`SystemMachine`]); `sightings.rs` keeps each worker's last sighting as
//! a file of its own ([`FileSightings`]); `ssh_config.rs` reads the
//! person's `~/.ssh/config` and what it includes ([`HomeSshConfig`]).
mod machine;
mod sightings;
#[cfg_attr(not(unix), allow(dead_code, reason = "the Unix gateway composes it"))]
mod ssh_config;

pub(crate) use machine::SystemMachine;
pub(crate) use sightings::FileSightings;
pub(crate) use ssh_config::HomeSshConfig;

use crate::devices::application::WallClock;

/// The process's wall clock.
pub(crate) struct SystemWallClock;

impl WallClock for SystemWallClock {
    fn now_ms(&self) -> u64 {
        nessa_sdk::infrastructure::clock::process_unix_ms()
    }
}
