//! One grant's command matchers, as Cedar JSON policies.
//!
//! The program name and argv pattern are facts in the policy JSON. They are
//! never written into Cedar source text.
//!
//! A deny matcher is a forbid. When the grant has any allow matcher, an
//! unlisted program is a forbid of everything those allows do not match, so a
//! deny of an allowed program still wins. When the grant has only denies, the
//! unlisted policy is a permit, and a deny still forbids the programs it names.

use cedar_policy::{Policy, PolicyId, PolicySet};
use serde_json::{json, Value};

use crate::application::PolicyUnavailable;
use crate::domain::{program_name, CommandMatcher, CommandRules, RuleEffect};

const PREFIX: &str = "command.rules.";
const GRANT: &str = "NessaDirectory::Grant";
const ACTION: &str = "NessaDirectory::Action";
const COMMAND: &str = "NessaDirectory::Command";
const COMMAND_ACTION: &str = "directory.command";

pub(super) fn reserved(id: &str) -> bool {
    id.starts_with(PREFIX)
}

pub(super) fn policies(rules: &CommandRules) -> Result<PolicySet, PolicyUnavailable> {
    let mut set = PolicySet::new();
    let mut allows = Vec::new();
    for (index, rule) in rules.rules().iter().enumerate() {
        let effect = match rule.effect() {
            RuleEffect::Allow => {
                allows.push(rule);
                "permit"
            }
            RuleEffect::Deny => "forbid",
        };
        let id = format!("{PREFIX}{}{index}", effect_name(rule));
        set.add(policy(&id, effect, json!([condition(matcher_expr(rule))]))?)
            .map_err(|_| PolicyUnavailable)?;
    }
    if !allows.is_empty() {
        let matched = or_all(allows.iter().copied().map(matcher_expr).collect());
        set.add(policy(
            &format!("{PREFIX}unlisted"),
            "forbid",
            json!([condition(json!({ "!": { "arg": matched } }))]),
        )?)
        .map_err(|_| PolicyUnavailable)?;
    } else if rules.constrains_commands() {
        set.add(policy(&format!("{PREFIX}unlisted"), "permit", json!([]))?)
            .map_err(|_| PolicyUnavailable)?;
    }
    Ok(set)
}

fn effect_name(rule: &CommandMatcher) -> &'static str {
    match rule.effect() {
        RuleEffect::Allow => "allow.",
        RuleEffect::Deny => "deny.",
    }
}

fn policy(id: &str, effect: &str, conditions: Value) -> Result<Policy, PolicyUnavailable> {
    let value = json!({
        "annotations": { "id": id },
        "effect": effect,
        "principal": { "op": "is", "entity_type": GRANT },
        "action": { "op": "==", "entity": { "type": ACTION, "id": COMMAND_ACTION } },
        "resource": { "op": "is", "entity_type": COMMAND },
        "conditions": conditions,
    });
    Policy::from_json(Some(PolicyId::new(id)), value).map_err(|_| PolicyUnavailable)
}

fn condition(body: Value) -> Value {
    json!({ "kind": "when", "body": body })
}

fn matcher_expr(rule: &CommandMatcher) -> Value {
    let program = equals(
        attr("resource", "programName"),
        json!({ "Value": program_name(rule.program()) }),
    );
    match rule.argv() {
        Some(argv) => and(program, like(attr("context", "argv"), argv.as_str())),
        None => program,
    }
}

fn or_all(mut exprs: Vec<Value>) -> Value {
    let mut acc = exprs.pop().expect("an allow matcher");
    while let Some(next) = exprs.pop() {
        acc = json!({ "||": { "left": next, "right": acc } });
    }
    acc
}

fn and(left: Value, right: Value) -> Value {
    json!({ "&&": { "left": left, "right": right } })
}

fn equals(left: Value, right: Value) -> Value {
    json!({ "==": { "left": left, "right": right } })
}

fn attr(var: &str, name: &str) -> Value {
    json!({ ".": { "left": { "Var": var }, "attr": name } })
}

fn like(left: Value, pattern: &str) -> Value {
    json!({
        "like": {
            "left": left,
            "pattern": like_pattern(pattern),
        }
    })
}

fn like_pattern(pattern: &str) -> Value {
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
