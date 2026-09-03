//! Runtime configuration — sole gateway for process env and stage policy.
//!
//! Nothing else in the crate reads `std::env` or `env!` for config. Stage drives
//! auth policy (dev may use a default token; ci/alpha/prod require `NESSA_TOKEN`).
//! Tests inject config via `MockEnv` instead of mutating the real environment.
//!
//! ```text
//! NESSA_STAGE / NESSA_HOST / NESSA_PORT / NESSA_TOKEN
//!        │
//!        ▼
//!   Environment::load(source)
//!        │
//!        ├──► composition (bind addr, stage log)
//!        └──► app::AppState (expected auth token, version)
//! ```

mod config;
mod environment;
mod error;
mod source;
mod stage;

pub use config::key::{HOST, PORT, STAGE, TOKEN};
pub use config::VERSION;
pub use environment::Environment;
pub use error::EnvironmentError;
pub use source::MockEnv;
pub use stage::Stage;
