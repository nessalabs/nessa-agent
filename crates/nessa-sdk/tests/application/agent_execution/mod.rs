//! Application sessions, execution projections, and attributed permission decisions.
//!
//! ```text
//! application tests -> substituted backends -> application contracts
//! provider_identity -> SessionChange::ProviderIdentity fold, RecordStorage replay, restore
//! ```
//! Arrows show which test layer exercises each feature.

mod agents;
mod executions;
mod hooks;
mod permissions;
mod provider_identity;
mod providers;
mod scheduling;
mod support;
