//! Command-line surface. Commands call the gateway; they never open its registry.
//!
//! ```text
//! entrypoint ──▶ application::invoke ──▶ Gateway::call ──▶ LocalProduct
//! infrastructure::mcp ──┘
//! ```
//! Arrows are calls. `invoke` is the one product call. The gateway's Cedar
//! check is the only authorization, and it sees the credential that signed in.
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
#[cfg(test)]
#[path = "../../tests/cli/surface.rs"]
mod surface_tests;
