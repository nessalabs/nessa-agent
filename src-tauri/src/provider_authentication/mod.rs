//! Provider-owned login flows launched by a trusted conversation window.
//!
//! `contracts` owns the closed provider values and injected login port;
//! `commands` owns window admission and launch coordination. `native` implements
//! the port at the process boundary. Composition supplies that implementation.
//!
//! ```text
//! bundled window --> commands --> contracts::ProviderLogin --> native terminal
//! ```
//! Arrows are calls. Commands acknowledge only opening login; credentials and
//! the authentication result remain with the provider CLI. Named feature tests
//! in `src-tauri/tests/provider_authentication` verify admission and launching.

pub(crate) mod commands;
mod contracts;
mod native;

pub use contracts::{LoginFailure, Provider, ProviderLogin};
pub use native::NativeProviderLogin;
