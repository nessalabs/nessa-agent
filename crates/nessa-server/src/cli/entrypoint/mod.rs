//! Parse the user-facing command contract before any network or storage effects.
mod arguments;
pub use arguments::{parse, Command, LocalProvisioning, HELP};
