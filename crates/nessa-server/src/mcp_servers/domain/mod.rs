//! Pure rules: what stands in for a configured server, which stand-ins are
//! let through, and the token that ties one to its conversation.
mod session_token;
mod stand_in;
pub use session_token::{session_token, TokenDigest, SESSION_VARIABLE};
pub use stand_in::{
    admit, configuration_digest, relay_arguments, StandInRefusal, RELAY_SUBCOMMAND,
};
