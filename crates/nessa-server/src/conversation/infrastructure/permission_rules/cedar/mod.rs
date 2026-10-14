//! Cedar adapter for remembered permission rules.
//!
//! A stored rule is one JSON policy. Its id is `permission.rule.` plus the
//! row's id, so the deciding policy names the rule. A permit allows, a forbid
//! denies, and a decision no stored policy took is ask — Cedar's default deny
//! with an empty reason — rather than a refusal of the tool. Creating,
//! listing, and revoking are the checked-in policies, which permit only
//! `Nessa::Owner`.
//!
//! ```text
//! StoredPermissionRule ──▶ JSON policy ──▶ Cedar ──▶ RuleSettlement
//! RulePrincipal + RuleChange ──▶ Cedar ──▶ may
//! ```

mod evaluator;

pub(crate) use evaluator::CedarPermissionRuleEvaluator;
