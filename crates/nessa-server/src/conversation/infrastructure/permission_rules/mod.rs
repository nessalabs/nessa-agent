//! Remembered permission rules.
//!
//! A rule is one tool in one workspace. Cedar decides whether it answers a
//! review and whether the caller may change the rules (`cedar/`). The file
//! is `permission-rules.sqlite3`, not the conversation catalogue. A row that
//! is not a rule is skipped; the rest still answer. A rule change and its
//! audit row commit in that same file, in one transaction.
//!
//! ```text
//! PermissionRules ─┐
//! PermissionRuleCatalog ─┴─▶ PermissionMemory ─▶ permission-rules.sqlite3
//!                                        │
//!                                        └─▶ CedarPermissionRuleEvaluator
//! ```

mod cedar;
mod memory;
mod store;

pub use memory::{PermissionMemory, RulesOpenError};
