//! Gateway integration of the reusable vault, including explicit legacy imports.
//! `product/vault` coordinates authenticated commands through application ports;
//! `infrastructure` is the sole one-time reader of old GitHub OS entries.
pub mod application;
pub mod infrastructure;

#[cfg(test)]
#[path = "../../tests/vault/migration.rs"]
mod migration_tests;
