//! Env var names, default values, and build-time version for [`super::environment`].

/// Process environment variable names (`NESSA_*`).
pub mod key {
    pub const DATA_DIR: &str = "NESSA_DATA_DIR";
    pub const INSTANCE: &str = "NESSA_INSTANCE";
    pub const HOME: &str = "HOME";
    pub const UPTIME_BACKEND: &str = "NESSA_UPTIME_BACKEND";
    pub const UPTIME_FIXED_MS: &str = "NESSA_UPTIME_FIXED_MS";
    pub const STAGE: &str = "NESSA_STAGE";
    pub const HOST: &str = "NESSA_HOST";
    pub const PORT: &str = "NESSA_PORT";
}

/// Fallback values when a var is unset.
pub mod default {
    pub const HOST: &str = "127.0.0.1";
    pub const PORT: u16 = 7420;
}

/// Crate version from `Cargo.toml`. Sole `env!` usage in this crate.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
