//! Background command ownership outside a provider attachment.
//!
//! `admission -> store -> runner -> terminal store -> internal continuation`.
//! Arrows are ordered effects. A caller disappearing does not own cancellation.
//! The conversation service supplies authority and owns the final continuation.

mod ports;
mod records;

pub use ports::{
    BackgroundCallMatcher, BackgroundCleanupFailure, BackgroundRun, BackgroundRunner,
    BackgroundStore, BackgroundTaskError,
};
pub use records::{BackgroundRecord, BackgroundSource};

/// Bounds live owners and unconfirmed ownership retained from earlier runs.
pub const MAX_BACKGROUND_TASKS: usize = 32;
pub const MAX_CONVERSATION_BACKGROUND_TASKS: usize = 4;

/// The built-in server and operation identities used for native-call correlation.
pub const BACKGROUND_SERVER: &str = "nessa-background";
pub const BACKGROUND_START: &str = "start";
