//! Provider identities and cursor ordering, independent of transport and storage.
pub mod contract;
mod delivery;
pub use delivery::{Binding, Event, Failure, Page};
