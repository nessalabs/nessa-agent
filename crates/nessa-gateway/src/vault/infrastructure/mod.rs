//! Explicit one-time legacy source; normal vault access has no OS dependency.
mod legacy_github;
pub use legacy_github::LegacyGithubMigration;
