//! GitHub over HTTPS, the OS sign-in entry, the audit, and what agents
//! are given.
mod api;
mod app;
mod audit;
mod launch;
mod token_vault;

pub use api::GithubHttps;
pub use app::{GithubApp, GithubAppProblem};
pub use audit::DurableGithubAudit;
pub use launch::{variables, GithubLaunchCredentials};
pub use token_vault::GithubTokenVault;

mod vault_access;
pub use vault_access::GithubVaultAccess;
