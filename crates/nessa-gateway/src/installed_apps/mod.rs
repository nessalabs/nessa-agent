//! Installed static apps: the owner's private store of verified packages,
//! and the audit of every install and uninstall.
//!
//! ```text
//! apps.starter / apps.review ──▶ nessa-app-package verifier (nothing kept)
//! apps.install ──▶ InstalledApps ──▶ verify again, digest == reviewed?
//!                                      ├──▶ AppsAudit "requested"
//!                                      ├──▶ AppStore (one transaction)
//!                                      └──▶ AppsAudit "installed" | "failed"
//! apps.installed / apps.read ──▶ AppStore (bytes verified on every load)
//! apps.uninstall ──▶ InstalledApps ──▶ audit, AppStore remove, audit
//! ```
//!
//! Arrows are calls. Cedar admits every method as `credential.manage`, so
//! only the owner's own window reaches here. The gateway is the one
//! authority on what is installed: a window renders only what `apps.read`
//! hands it, and installs only the bytes whose digest it showed the owner.
pub mod application;
pub mod infrastructure;

#[cfg(test)]
#[path = "../../tests/installed_apps/mod.rs"]
mod tests;
