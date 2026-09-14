//! Application sessions, execution projections, and attributed permission decisions.
//!
//! ```text
//! application tests -> substituted backends -> application contracts
//! ```
//! Arrows show which test layer exercises each feature.

mod agents;
mod executions;
mod hooks;
mod permissions;
mod providers;
mod scheduling;
mod support;
