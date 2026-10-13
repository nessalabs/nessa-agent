//! Remembered permission rules, and the notices a waiting review raises.
//!
//! A rule is one tool in one workspace. Cedar decides whether it answers a
//! review and whether the caller may change the rules (`cedar/`). The file
//! is `permission-rules.sqlite3`, not the conversation catalogue. A row that
//! is not a rule is skipped; the rest still answer.
//!
//! ```text
//! PermissionRules ─┐
//! ReviewNotices    ├─▶ PermissionMemory ─▶ permission-rules.sqlite3
//! PermissionRuleCatalog ─┘                         │
//!                                                  └─▶ CedarPermissionRuleEvaluator
//! ```

mod cedar;
mod memory;
mod store;

pub use memory::{PermissionMemory, PublishedReview};
