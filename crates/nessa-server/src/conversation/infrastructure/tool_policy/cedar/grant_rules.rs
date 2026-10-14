//! A grant's allow and deny patterns, as Cedar JSON policies scoped to it.
//!
//! The pattern text goes into a `like` pattern's literal parts; it is never
//! formatted into policy text, so no character in it is read as syntax.
//!
//! A pattern matches a tool's name from any source: `Bash` denies Claude's
//! `Bash`, not Codex's `exec_command` or the gateway's `run`. A deny list is
//! therefore not "no shell"; the `no_shell` preset is.

use cedar_policy::{Policy, PolicyId};
use serde_json::{json, Value};

use crate::conversation::application::ToolPolicyUnavailable;
use crate::conversation::domain::ToolRules;

use super::evaluator::{TOOL, TOOL_CALL};
use super::request::{ACTION, GRANT};

/// The prefix of every grant rule's id; no checked-in policy uses it.
pub(super) const GRANT_RULES: &str = "tool.rules.";

/// `rules`, for `grant`, as policies: a permit per allow pattern, a forbid
/// per deny pattern, and a permit of every tool when there is no allow
/// pattern.
pub(super) fn grant_policies(
    grant: &str,
    rules: &ToolRules,
) -> Result<Vec<Policy>, ToolPolicyUnavailable> {
    let mut policies = Vec::new();
    for (index, pattern) in rules.allow().iter().enumerate() {
        policies.push(policy(
            format!("{GRANT_RULES}allow.{index}"),
            "permit",
            grant,
            Some(pattern),
        )?);
    }
    if rules.allow().is_empty() {
        policies.push(policy(
            format!("{GRANT_RULES}unlisted"),
            "permit",
            grant,
            None,
        )?);
    }
    for (index, pattern) in rules.deny().iter().enumerate() {
        policies.push(policy(
            format!("{GRANT_RULES}deny.{index}"),
            "forbid",
            grant,
            Some(pattern),
        )?);
    }
    Ok(policies)
}

fn policy(
    id: String,
    effect: &str,
    grant: &str,
    pattern: Option<&str>,
) -> Result<Policy, ToolPolicyUnavailable> {
    let conditions = match pattern {
        Some(pattern) => json!([{
            "kind": "when",
            "body": {
                "like": {
                    "left": { ".": { "left": { "Var": "resource" }, "attr": "name" } },
                    "pattern": like(pattern),
                }
            }
        }]),
        None => json!([]),
    };
    let policy = json!({
        "annotations": { "id": id },
        "effect": effect,
        "principal": { "op": "==", "entity": { "type": GRANT, "id": grant } },
        "action": { "op": "==", "entity": { "type": ACTION, "id": TOOL_CALL } },
        "resource": { "op": "is", "entity_type": TOOL },
        "conditions": conditions,
    });
    Policy::from_json(Some(PolicyId::new(id)), policy).map_err(|_| ToolPolicyUnavailable)
}

/// `pattern` as `like` elements: each `*` a wildcard, the text between them
/// literal.
fn like(pattern: &str) -> Value {
    let mut elements = Vec::new();
    for (index, literal) in pattern.split('*').enumerate() {
        if index > 0 {
            elements.push(json!("Wildcard"));
        }
        if !literal.is_empty() {
            elements.push(json!({ "Literal": literal }));
        }
    }
    Value::Array(elements)
}
