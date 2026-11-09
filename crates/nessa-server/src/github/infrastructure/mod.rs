//! GitHub over HTTPS, the encrypted sign-in entry, the audit, and what agents
//! are given.
mod api;
mod app;
mod audit;
mod token_vault;

pub use api::GithubHttps;
pub use app::{GithubApp, GithubAppProblem};
pub use audit::DurableGithubAudit;
pub use token_vault::{remove_plaintext_sign_in, GithubTokenVault};

mod vault_access;
pub use vault_access::GithubVaultAccess;
