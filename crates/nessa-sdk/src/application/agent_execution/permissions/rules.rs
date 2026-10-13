//! Scoped permission rules and the notices a waiting review raises.
//!
//! A rule answers a later review of one tool in one workspace. The host
//! stores it and decides it; these types do not. A notice names a review
//! that is still waiting, and is cleared when that review is answered,
//! cancelled, or settled by its deadline.
//!
//! ```text
//! owner turn ──arm──▶ PermissionRules::offers_remember
//! review ──settle──▶ live rule selects the once option (Rule basis)
//! review left pending ──waiting──▶ ReviewNotices
//! answer | cancel | deadline ──clear──▶ ReviewNotices
//! ```

use crate::application::agent_execution::permissions::{ActionContext, ApprovalRuleReference};
use crate::domain::agent_execution::executions::ExecutionId;
use crate::domain::agent_execution::permissions::{PermissionEffect, PermissionId};

/// Option id of Nessa's "always allow in this project" choice.
pub const REMEMBER_ALLOW: &str = "nessa.remember.allow";
/// Option id of Nessa's "always deny in this project" choice.
pub const REMEMBER_DENY: &str = "nessa.remember.deny";

/// Whether `id` is one of Nessa's remember choices.
pub fn is_remember_option(id: &str) -> bool {
    id == REMEMBER_ALLOW || id == REMEMBER_DENY
}

/// The effect a remember option stores. `None` when `id` is not one.
pub fn remember_effect(id: &str) -> Option<PermissionEffect> {
    match id {
        REMEMBER_ALLOW => Some(PermissionEffect::Allow),
        REMEMBER_DENY => Some(PermissionEffect::Deny),
        _ => None,
    }
}

/// A live rule that can answer a review, and the effect it selects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettledRule {
    reference: ApprovalRuleReference,
    effect: PermissionEffect,
}

impl SettledRule {
    /// A rule the host has already stored. Construction does not store one.
    pub fn new(reference: ApprovalRuleReference, effect: PermissionEffect) -> Self {
        Self { reference, effect }
    }

    /// The rule, its revision, and who granted it.
    pub fn reference(&self) -> &ApprovalRuleReference {
        &self.reference
    }

    /// Allow or deny. The review is answered with the once option of this effect.
    pub fn effect(&self) -> PermissionEffect {
        self.effect
    }
}

/// Who is answering, as the host verified it.
///
/// Cedar admits a remembered rule for [`Self::Owner`] and refuses
/// [`Self::Grant`]. A missing caller is not treated as the owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionCaller {
    /// The conversation's owner.
    Owner,
    /// Someone holding a share grant.
    Grant,
}

/// What remembering a choice asks the host to store.
#[derive(Clone, Debug)]
pub struct RememberRequest<'a> {
    /// Absolute workspace path the rule is scoped to.
    pub workspace: &'a str,
    /// Harness source: `claude` or `codex`.
    pub source: &'a str,
    /// Tool name the review named.
    pub tool: &'a str,
    /// Allow or deny.
    pub effect: PermissionEffect,
    /// The person who clicked. The rule records this grantor.
    pub granted_by: &'a ActionContext,
    /// Who is asking to store the rule. The host verified this.
    pub caller: PermissionCaller,
    /// The execution whose review offered the choice.
    pub execution_id: &'a ExecutionId,
}

/// Why a remember choice was not stored. The review stays pending.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RememberError {
    /// The caller may not create a rule, or the turn was not the owner's.
    Refused,
    /// The store or the policy could not be asked. Nothing was written.
    Unavailable,
}

/// Host store of scoped permission rules.
///
/// `settle` returns a live rule or nothing. Nothing means ask the person.
/// A failure to read the store is nothing, so a broken store never allows
/// or denies a tool by itself. `remember` is the opposite: a failure leaves
/// the review pending and writes nothing.
pub trait PermissionRules: Send + Sync {
    /// A live rule for this tool in this workspace, if one should answer it.
    fn settle(&self, workspace: &str, source: &str, tool: &str) -> Option<SettledRule>;

    /// Whether this execution may be offered Nessa's remember choices.
    fn offers_remember(&self, execution: &ExecutionId) -> bool;

    /// Record that `execution` is an owner turn (`owner`) or is not.
    fn arm(&self, execution: &ExecutionId, owner: bool);

    /// Record that the answer identified by `execution` and `request_id` is
    /// `caller`. The host sets this before that answer is queued. A later note
    /// for the same key replaces this one. Reading it in [`Self::answerer`]
    /// removes it, so a later answer is not attributed to an earlier caller.
    fn note_answerer(&self, execution: &ExecutionId, request_id: &str, caller: PermissionCaller) {
        let _ = (self, execution, request_id, caller);
    }

    /// The caller noted for this answer, if the host set one.
    ///
    /// `None` when it did not. Callers must not treat `None` as the owner.
    /// Reading removes the note.
    fn answerer(&self, execution: &ExecutionId, request_id: &str) -> Option<PermissionCaller> {
        let _ = (self, execution, request_id);
        None
    }

    /// Store a rule for `request.caller`. The review stays pending on error.
    /// Cedar admits or refuses that caller; the owner is not assumed.
    ///
    /// The revision stays only after [`Self::acknowledge`]. Until then,
    /// [`Self::withdraw`] removes a fresh rule or restores the revision a
    /// replacement wrote over. A host that stores nothing may leave both as
    /// they are.
    fn remember(&self, request: RememberRequest<'_>) -> Result<SettledRule, RememberError>;

    /// The selection that stored `rule` was recorded. Withdrawing it afterwards
    /// does not remove that revision or restore what it replaced.
    fn acknowledge(&self, rule: &ApprovalRuleReference) {
        let _ = (self, rule);
    }

    /// The selection that stored `rule` was not recorded. Remove that revision.
    ///
    /// A fresh rule is deleted. A replacement returns to the revision it
    /// replaced. A revision that was already acknowledged, already revoked, or
    /// replaced by a later remember is left as it is. [`RememberError::Unavailable`]
    /// means the row is still the unrecorded revision.
    fn withdraw(&self, rule: &ApprovalRuleReference) -> Result<(), RememberError> {
        let _ = (self, rule);
        Ok(())
    }
}

/// No rules, and no execution is offered a remember choice.
#[derive(Clone, Copy, Debug, Default)]
pub struct IdlePermissionRules;

impl PermissionRules for IdlePermissionRules {
    fn settle(&self, _: &str, _: &str, _: &str) -> Option<SettledRule> {
        None
    }
    fn offers_remember(&self, _: &ExecutionId) -> bool {
        false
    }
    fn arm(&self, _: &ExecutionId, _: bool) {}
    fn remember(&self, _: RememberRequest<'_>) -> Result<SettledRule, RememberError> {
        Err(RememberError::Refused)
    }
}

/// A review that is waiting on a person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewNotice {
    /// Conversation the review belongs to. The SDK session key is that id.
    pub conversation_id: String,
    /// Execution that owns the review.
    pub execution_id: ExecutionId,
    /// Pending permission identity.
    pub permission_id: PermissionId,
    /// Tool the review named.
    pub tool: String,
}

/// Notices a waiting review raises, and clears when the review ends.
pub trait ReviewNotices: Send + Sync {
    /// `notice` is waiting. A second call for the same review replaces it.
    fn waiting(&self, notice: &ReviewNotice);
    /// The review was answered, cancelled, or settled by its deadline.
    fn clear(&self, execution: &ExecutionId, permission: &PermissionId);
}

/// No notices.
#[derive(Clone, Copy, Debug, Default)]
pub struct IdleReviewNotices;

impl ReviewNotices for IdleReviewNotices {
    fn waiting(&self, _: &ReviewNotice) {}
    fn clear(&self, _: &ExecutionId, _: &PermissionId) {}
}
