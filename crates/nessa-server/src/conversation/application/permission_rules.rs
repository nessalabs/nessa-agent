//! Scoped permission rules: one tool in one workspace, granted by the owner.
//!
//! The store and the Cedar adapter live under `infrastructure/permission_rules`.
//! A miss asks the person. A failure to read the rules also asks: a broken
//! store never allows or denies a tool on its own. Creating, listing, and
//! revoking are the other way: a failure refuses the change and writes nothing.
//!
//! ```text
//! stored rows ──▶ PermissionRulePolicyEvaluator ──▶ settle | may
//! owner turn arm ──▶ PermissionRules::offers_remember
//! remember / revoke / list ──▶ owner only
//! ```

use nessa_sdk::application::agent_execution::permissions::ActionContext;
use nessa_sdk::domain::agent_execution::permissions::PermissionEffect;

/// Who is asking. A grant may be settled by a rule the owner stored. Only
/// the owner may create, list, or revoke one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RulePrincipal {
    /// The conversation's owner.
    Owner,
    /// Someone holding a share grant.
    Grant,
}

/// A change the owner may make, and a shared user may not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleChange {
    /// Store a rule.
    Grant,
    /// Delete a rule.
    Revoke,
    /// Read the rules.
    List,
}

/// One stored rule the evaluator is asked about. The text is the row; the
/// evaluator does not trust a Rust verdict beside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredPermissionRule {
    /// Stable id. A replacement keeps it and bumps `revision`.
    pub id: String,
    /// Revision of this id. The audit records the one that answered.
    pub revision: String,
    /// Workspace path the rule is scoped to.
    pub workspace: String,
    /// `claude` or `codex`.
    pub source: String,
    /// Tool name.
    pub tool: String,
    /// Allow or deny.
    pub effect: PermissionEffect,
    /// Who granted this revision.
    pub granted_by: ActionContext,
}

/// What a live rule does to a review. `Ask` means no rule decided it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleSettlement {
    /// No rule matched. Ask the person.
    Ask,
    /// A permit rule matched. `rule_id` is that rule.
    Allow { rule_id: String },
    /// A forbid rule matched. `rule_id` is that rule.
    Deny { rule_id: String },
}

/// The policy could not be asked. Callers that settle a review ask the
/// person. Callers that change a rule refuse the change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PermissionRuleUnavailable;

/// Whether stored rules answer a review, and whether the caller may change them.
pub trait PermissionRulePolicyEvaluator: Send + Sync {
    /// What `rules` decide for this tool in this workspace. A policy error is
    /// [`PermissionRuleUnavailable`], which the caller treats as ask.
    fn settle(
        &self,
        workspace: &str,
        source: &str,
        tool: &str,
        rules: &[StoredPermissionRule],
    ) -> Result<RuleSettlement, PermissionRuleUnavailable>;

    /// Whether `principal` may make `change`. A policy error is unavailable.
    fn may(
        &self,
        principal: RulePrincipal,
        change: RuleChange,
    ) -> Result<bool, PermissionRuleUnavailable>;
}

/// A rule as a person lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedPermissionRule {
    /// Stable id.
    pub id: String,
    /// Current revision.
    pub revision: String,
    /// Workspace path.
    pub workspace: String,
    /// `claude` or `codex`.
    pub source: String,
    /// Tool name.
    pub tool: String,
    /// `allow` or `deny`.
    pub effect: &'static str,
}

/// Why a list or a revoke did not happen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleCatalogError {
    /// The caller may not list or revoke. Nothing was read or deleted.
    Refused,
    /// The store or the policy could not be asked. Nothing was deleted.
    Unavailable,
}

/// List and revoke, for the owner's surfaces.
pub trait PermissionRuleCatalog: Send + Sync {
    /// The valid rules, in the order they were stored.
    fn list(&self, principal: RulePrincipal)
        -> Result<Vec<ListedPermissionRule>, RuleCatalogError>;
    /// Delete `id`. `Ok(false)` when no rule has it. A shared user is refused.
    fn revoke(&self, id: &str, principal: RulePrincipal) -> Result<bool, RuleCatalogError>;
}
