//! Helper services the gateway keeps running beside itself, such as the web
//! tunnel's program (`docs/design/auth/tunnel.md`).
//!
//! ```text
//! composition ──▶ application::ServiceSupervisor (one per service: start or take back, ready, restart)
//!             ──▶ application::Service: what to run, its key, how to tell it is ready (e.g. tunnel)
//!             ──▶ infrastructure::KeptHost: the processes, left running across the gateway's runs
//! ```
//!
//! Arrows are construction and the calls each makes. A service decides what
//! it runs; the host owns each process and whether it outlives the run; the
//! supervisor owns only the policy of starting again. The host stands in for
//! shepherd's kept processes until those land.
// Only a Unix gateway runs helper services today (native linking is Unix
// only); elsewhere the policy compiles and nothing starts it.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) mod application;
#[cfg(unix)]
pub(crate) mod infrastructure;
