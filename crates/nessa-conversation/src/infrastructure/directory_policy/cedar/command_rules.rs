//! One grant's command matchers, as Cedar policies.
//!
//! A deny matcher is a forbid. When the grant has any allow matcher, an
//! unlisted program is a forbid of everything those allows do not match, so a
//! deny of an allowed program still wins. When the grant has only denies, the
//! unlisted policy is a permit, and a deny still forbids the programs it names.

use std::str::FromStr;

use cedar_policy::{PolicyId, PolicySet};

use crate::application::PolicyUnavailable;
use crate::domain::{program_name, CommandMatcher, CommandRules, RuleEffect};

const PREFIX: &str = "command.rules.";

pub(super) fn reserved(id: &str) -> bool {
    id.starts_with(PREFIX)
}

pub(super) fn policies(rules: &CommandRules) -> Result<PolicySet, PolicyUnavailable> {
    let mut text = String::new();
    let mut allows = Vec::new();
    for (index, rule) in rules.rules().iter().enumerate() {
        let effect = match rule.effect() {
            RuleEffect::Allow => {
                allows.push(rule.clone());
                "permit"
            }
            RuleEffect::Deny => "forbid",
        };
        let id = format!("{PREFIX}{}{index}", effect_name(rule));
        text.push_str(&format!("@id(\"{id}\")\n{}\n", one(effect, rule)));
    }
    if !allows.is_empty() {
        text.push_str(&format!(
            "@id(\"{PREFIX}unlisted\")\n{}\n",
            unlisted(&allows)
        ));
    } else if rules.constrains_commands() {
        text.push_str(&format!(
            "@id(\"{PREFIX}unlisted\")\npermit (principal is NessaDirectory::Grant, action == NessaDirectory::Action::\"directory.command\", resource is NessaDirectory::Command);\n"
        ));
    }
    let parsed = PolicySet::from_str(&text).map_err(|_| PolicyUnavailable)?;
    let mut set = PolicySet::new();
    for policy in parsed.policies() {
        let Some(id) = policy.annotation("id") else {
            return Err(PolicyUnavailable);
        };
        set.add(policy.new_id(PolicyId::new(id)))
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

fn one(effect: &str, rule: &CommandMatcher) -> String {
    let program = cedar_string(rule.program());
    let mut when = format!("resource.programName == {program}");
    if let Some(argv) = rule.argv() {
        when.push_str(" && context.argv like ");
        when.push_str(&cedar_string(argv.as_str()));
    }
    format!(
        "{effect} (principal is NessaDirectory::Grant, action == NessaDirectory::Action::\"directory.command\", resource is NessaDirectory::Command) when {{ {when} }};"
    )
}

fn unlisted(allows: &[CommandMatcher]) -> String {
    let matched = allows
        .iter()
        .map(|rule| {
            let program = cedar_string(program_name(rule.program()));
            match rule.argv() {
                Some(argv) => format!(
                    "(resource.programName == {program} && context.argv like {})",
                    cedar_string(argv.as_str())
                ),
                None => format!("resource.programName == {program}"),
            }
        })
        .collect::<Vec<_>>()
        .join(" || ");
    format!(
        "forbid (principal is NessaDirectory::Grant, action == NessaDirectory::Action::\"directory.command\", resource is NessaDirectory::Command) when {{ !({matched}) }};"
    )
}

fn cedar_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}
