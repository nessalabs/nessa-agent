//! The domain defines model facts and the rules that keep them valid.
//! Application code calls these rules; the domain does not call out to other layers.
//! Keeping this boundary pure lets the same rules serve a desktop host or a CLI.
//!
//! ```text
//! infrastructure --> application --> domain
//!                                      |
//!                                      +-- model_metadata --> common
//! ```
//! Arrows mean "depends on". More domain features belong beside model_metadata.

pub mod common;
pub mod effective_capabilities;
pub mod model_metadata;
