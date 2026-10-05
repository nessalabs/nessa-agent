//! Provider-owned login flows launched by a trusted conversation window.
//!
//! `contracts` owns the closed provider values and injected login port;
//! `commands` owns window admission and launch coordination. `platform` implements
//! the port at the process boundary. Composition supplies the selected adapter.
//!
//! ```text
//! bundled window --> commands --> contracts::ProviderLogin --> platform launcher
//! ```
//! Arrows are calls. Commands acknowledge only opening login; credentials and
//! the authentication result remain with the provider CLI. Named feature tests
//! in `src-tauri/tests/provider_authentication` verify command admission; platform tests verify launcher behavior.

pub(crate) mod commands;
mod contracts;

pub use contracts::{LoginFailure, Provider, ProviderLogin};
