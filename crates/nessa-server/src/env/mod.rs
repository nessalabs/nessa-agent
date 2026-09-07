//! Runtime configuration — sole gateway for process env and stage policy.
//!
//! Nothing else in the crate reads `std::env` or `env!` for config. Stage drives
//! runtime behavior. Serving requires the local auth registry in every stage;
//! Tests inject config via `MockEnv` instead of mutating the real environment.
//!
//! ```text
//! NESSA_STAGE / NESSA_HOST / NESSA_PORT
//!        │
//!        ▼
//!   Environment::load(source)
//!        │
//!        ├──► composition (bind addr, stage log)
//!        └──► app::AppState (stage, version, uptime)
//! ```

mod config;
mod environment;
mod error;
mod source;
mod stage;

pub use config::key::{HOST, PORT, STAGE};
pub use config::VERSION;
pub use environment::Environment;
pub use error::EnvironmentError;
pub use source::MockEnv;
pub use stage::Stage;

mod backend;
pub use backend::UptimeBackend;

mod paths;
