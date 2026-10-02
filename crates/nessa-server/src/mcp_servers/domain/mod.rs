//! Pure rules: what stands in for a configured server, and which stand-ins
//! are let through.
mod stand_in;
pub use stand_in::{
    admit, configuration_digest, relay_arguments, StandInRefusal, RELAY_SUBCOMMAND,
};
