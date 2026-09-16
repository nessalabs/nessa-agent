//! Desktop gateway context: application coordinates a registered background service;
//! infrastructure implements native registration, readiness and agent-stop requests.
//!
//! ```text
//! main (composition) -> application::Gateway -> application::GatewayHost
//! infrastructure::current() -----------------> GatewayHost implementation
//! ```
//! Arrows mean construction or calls. The desktop does not own gateway lifetime.
pub mod application;
pub mod infrastructure;
