//! Policy answers without any persistence or claim of audited authorization.

use cedar_policy::{
    Authorizer, Context, Decision, Entities, EntityUid, Policy, PolicyId, PolicySet, Request,
    Schema, ValidationMode, Validator,
};
use serde_json::{json, Value};

use crate::application::{DirectoryOperation, DirectoryPolicyAnswer, DirectoryPolicyError};
use crate::domain::{DirectoryGrantState, DirectoryRuleEffect};

struct DirectoryPolicyFacts<'a> {
    operation: &'a str,
    within: bool,
    enforced_exec_boundary: bool,
    program: &'a str,
    argv: &'a [String],
}

/// Observe policy for an already selected resource and bounded host facts.
/// This does not authenticate a caller, select scope, authorize an effect, or audit.
pub fn observe_directory_policy(
    state: DirectoryGrantState,
    operation: &DirectoryOperation,
) -> Result<DirectoryPolicyAnswer, DirectoryPolicyError> {
    if !operation.valid()
        || matches!(
            &state,
            DirectoryGrantState::Live { revision, .. }
                | DirectoryGrantState::Revoked { revision } if *revision <= 0
        )
    {
        return Err(DirectoryPolicyError::InvalidFacts);
    }
    let (operation, within, enforced_exec_boundary, program, argv) = match operation {
        DirectoryOperation::Read { within } => ("read", *within, false, "", &[][..]),
        DirectoryOperation::Write { within } => ("write", *within, false, "", &[][..]),
        DirectoryOperation::Exec {
            within,
            enforced_boundary,
            program,
            argv,
        } => (
            "exec",
            *within,
            *enforced_boundary,
            program.as_str(),
            argv.as_slice(),
        ),
    };
    evaluate(
        state,
        &DirectoryPolicyFacts {
            operation,
            within,
            enforced_exec_boundary,
            program,
            argv,
        },
    )
    .map_err(|()| DirectoryPolicyError::Unavailable)
}

fn evaluate(
    state: DirectoryGrantState,
    facts: &DirectoryPolicyFacts<'_>,
) -> Result<DirectoryPolicyAnswer, ()> {
    let (schema, _) =
        Schema::from_cedarschema_str(include_str!("schema.cedarschema")).map_err(|_| ())?;
    let mut policies: PolicySet = include_str!("policy.cedar").parse().map_err(|_| ())?;
    let mut capabilities = [false; 3];
    let live = if let DirectoryGrantState::Live { grant, .. } = state {
        capabilities = grant.capabilities().facts();
        for (index, rule) in grant.command_rules().iter().enumerate() {
            let effect = match rule.effect {
                DirectoryRuleEffect::Allow => "permit",
                DirectoryRuleEffect::Deny => "forbid",
            };
            let id = PolicyId::new(format!("directory.command.{index}.{effect}"));
            let mut conditions = vec![
                json!({"==":{"left":attr("operation"),"right":{"Value":"exec"}}}),
                json!({"==":{"left":attr("program"),"right":{"Value":rule.program}}}),
                attr("live"),
                attr("within"),
                attr("exec"),
                attr("enforced_exec_boundary"),
            ];
            if !rule.argv.is_empty() {
                conditions
                    .push(json!({"==":{"left":attr("argc"),"right":{"Value":rule.argv.len()}}}));
                for (position, pattern) in rule.argv.iter().enumerate() {
                    conditions.push(json!({"like":{
                        "left":attr(&format!("arg{position}")),
                        "pattern":like(pattern)
                    }}));
                }
            }
            let policy = Policy::from_json(Some(id), json!({
                "effect": effect,
                "principal": {"op":"All"}, "action": {"op":"All"}, "resource": {"op":"All"},
                "conditions":conditions.into_iter().map(|body|json!({"kind":"when","body":body})).collect::<Vec<_>>()
            })).map_err(|_| ())?;
            policies.add(policy).map_err(|_| ())?;
        }
        true
    } else {
        false
    };
    if !Validator::new(schema.clone())
        .validate(&policies, ValidationMode::Strict)
        .validation_passed()
    {
        return Err(());
    }
    let mut context = json!({
        "live":live,"within":facts.within,"enforced_exec_boundary":facts.enforced_exec_boundary,
        "read":capabilities[0],"write":capabilities[1],"exec":capabilities[2],
        "operation":facts.operation,"program":facts.program,"argc":facts.argv.len(),
    });
    for position in 0..crate::domain::MAX_DIRECTORY_ARGV_ITEMS {
        context[format!("arg{position}")] =
            json!(facts.argv.get(position).map_or("", String::as_str));
    }
    let entity = |value: &str| -> Result<EntityUid, ()> { value.parse().map_err(|_| ()) };
    let request = Request::new(
        entity("DirectorySubject::\"selected\"")?,
        entity("Action::\"directory\"")?,
        entity("DirectoryResource::\"selected\"")?,
        Context::from_json_value(context, None).map_err(|_| ())?,
        Some(&schema),
    )
    .map_err(|_| ())?;
    let answer = Authorizer::new().is_authorized(&request, &policies, &Entities::empty());
    if answer.diagnostics().errors().next().is_some() {
        return Err(());
    }
    let mut reasons: Vec<_> = answer
        .diagnostics()
        .reason()
        .map(ToString::to_string)
        .collect();
    reasons.sort_unstable();
    Ok(DirectoryPolicyAnswer {
        permitted: answer.decision() == Decision::Allow,
        reasons,
    })
}

fn attr(name: &str) -> Value {
    json!({".":{"left":{"Var":"context"},"attr":name}})
}

fn like(pattern: &str) -> Vec<Value> {
    let mut elements = Vec::new();
    for (index, literal) in pattern.split('*').enumerate() {
        if index > 0 {
            elements.push(json!("Wildcard"));
        }
        if !literal.is_empty() {
            elements.push(json!({"Literal":literal}));
        }
    }
    elements
}

#[cfg(test)]
#[path = "../../../../tests/conversation/directory_policy.rs"]
mod tests;
