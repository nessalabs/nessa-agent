//! GitHub provider inbox: a stage-selected cloud source and a private local copy.
//!
//! ```text
//! composition -> credentials / transport / SQLite
//!                       -> application::catch_up -> RelayStore transaction
//! Settings -> LiveEvents -> atomic selection / cleanup
//! GithubConnections -> credentials -> current account / repositories
//! CLI -> read local events; server -> watch / reconnect
//! ```
//! Arrows are construction or calls. Provider data never selects a principal,
//! conversation or command. The packaged host owns the build stage.
pub mod application;
pub mod domain;
pub mod infrastructure;

#[cfg(test)]
#[path = "../../tests/github_relay/inbox.rs"]
mod inbox_tests;
