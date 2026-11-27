//! The adapters the apps application runs on: the SQLite store of verified
//! packages, and the durable audit.
mod audit;
mod store;

pub use audit::DurableAppsAudit;
pub use store::SqliteAppStore;
