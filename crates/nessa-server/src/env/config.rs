//! Env var names, default values, and build-time version for [`super::environment`].

/// Process environment variable names (`NESSA_*`).
pub mod key {
    pub const STAGE: &str = "NESSA_STAGE";
    pub const HOST: &str = "NESSA_HOST";
    pub const PORT: &str = "NESSA_PORT";
    pub const TOKEN: &str = "NESSA_TOKEN";
}

/// Fallback values when a var is unset.
pub mod default {
    pub const HOST: &str = "127.0.0.1";
    pub const PORT: u16 = 7420;
    pub const DEV_AUTH_TOKEN: &str = "dev-token";
}

/// Crate version from `Cargo.toml`. Sole `env!` usage in this crate.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
