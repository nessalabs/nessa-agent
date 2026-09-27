//! Command-line surface. Commands call the gateway; they never open its registry.
//! `entrypoint -> application -> ports <- infrastructure` shows dependency direction.
pub mod application;
pub mod entrypoint;
pub mod infrastructure;

#[cfg(test)]
#[path = "../../tests/cli/application.rs"]
mod application_tests;
#[cfg(test)]
#[path = "../../tests/cli/arguments.rs"]
mod argument_tests;
#[cfg(test)]
#[path = "../../tests/cli/gateway.rs"]
mod gateway_tests;
