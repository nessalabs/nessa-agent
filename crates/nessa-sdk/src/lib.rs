//! Reusable agent contracts: model metadata, capability validation, execution domain objects, and injected ports.
#[cfg(test)]
extern crate self as nessa_sdk;
pub mod application;
pub mod domain;
pub mod infrastructure;

pub use application::agent_execution::agents::Agent;
