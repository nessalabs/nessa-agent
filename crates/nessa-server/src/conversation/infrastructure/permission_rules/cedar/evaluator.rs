//! Ask Cedar whether a stored rule answers a review, and whether a caller
//! may change the rules.

use cedar_policy::{
    Authorizer, Context, Decision, Entities, Entity, EntityId, EntityTypeName, EntityUid, Policy,
    PolicyId, PolicySet, Request, RestrictedExpression, Schema, ValidationMode, Validator,
};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::LazyLock;

use crate::conversation::application::{
    PermissionRulePolicyEvaluator, PermissionRuleUnavailable, RuleChange, RulePrincipal,
    RuleSettlement, StoredPermissionRule,
};
use nessa_sdk::domain::agent_execution::permissions::PermissionEffect;

const SCHEMA: &str = include_str!("schema.json");
const RULES: &str = include_str!("rules.cedar");
const PREFIX: &str = "permission.rule.";
const OWNER: &str = "Nessa::Owner";
const GRANT: &str = "Nessa::Grant";
const TOOL: &str = "Nessa::Tool";
const ACTION: &str = "Nessa::Action";

struct Bundle {
    schema: Schema,
    policies: PolicySet,
}

fn load() -> Result<Bundle, PermissionRuleUnavailable> {
    let schema = Schema::from_json_str(SCHEMA).map_err(|_| PermissionRuleUnavailable)?;
    let parsed = PolicySet::from_str(RULES).map_err(|_| PermissionRuleUnavailable)?;
    let mut policies = PolicySet::new();
    for policy in parsed.policies() {
        let id = policy
            .annotation("id")
            .ok_or(PermissionRuleUnavailable)?
            .to_owned();
        if id.starts_with(PREFIX) {
            return Err(PermissionRuleUnavailable);
        }
        policies
            .add(policy.new_id(PolicyId::new(&id)))
            .map_err(|_| PermissionRuleUnavailable)?;
    }
    let bundle = Bundle { schema, policies };
    validate(&bundle.schema, &bundle.policies)?;
    Ok(bundle)
}

fn validate(schema: &Schema, policies: &PolicySet) -> Result<(), PermissionRuleUnavailable> {
    let validation = Validator::new(schema.clone()).validate(policies, ValidationMode::Strict);
    if validation.validation_passed_without_warnings() {
        Ok(())
    } else {
        Err(PermissionRuleUnavailable)
    }
}

static BUNDLE: LazyLock<Result<Bundle, PermissionRuleUnavailable>> = LazyLock::new(load);

/// The Cedar evaluator for remembered permission rules.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CedarPermissionRuleEvaluator;

impl CedarPermissionRuleEvaluator {
    /// Load and strictly validate the checked-in schema and policies.
    ///
    /// Composition calls this when it builds the adapter, so a broken bundle
    /// refuses to start instead of failing closed on the first review.
    pub(in crate::conversation::infrastructure::permission_rules) fn validated(
    ) -> Result<Self, PermissionRuleUnavailable> {
        BUNDLE
            .as_ref()
            .map(|_| Self)
            .map_err(|_| PermissionRuleUnavailable)
    }
}

impl PermissionRulePolicyEvaluator for CedarPermissionRuleEvaluator {
    fn settle(
        &self,
        principal_kind: RulePrincipal,
        workspace: &str,
        source: &str,
        tool: &str,
        rules: &[StoredPermissionRule],
    ) -> Result<RuleSettlement, PermissionRuleUnavailable> {
        if rules.is_empty() {
            return Ok(RuleSettlement::Ask);
        }
        let bundle = BUNDLE.as_ref().map_err(|_| PermissionRuleUnavailable)?;
        let mut policies = bundle.policies.clone();
        for rule in rules {
            policies
                .add(rule_policy(rule)?)
                .map_err(|_| PermissionRuleUnavailable)?;
        }
        validate(&bundle.schema, &policies)?;
        let response = authorized(
            bundle,
            &policies,
            principal(principal_kind)?,
            "rule.settle",
            tool_entity(tool, source)?,
            context(workspace)?,
        )?;
        let decision = response.decision();
        let mut rule_ids: Vec<String> = response
            .diagnostics()
            .reason()
            .map(ToString::to_string)
            .filter_map(|id| id.strip_prefix(PREFIX).map(str::to_owned))
            .filter(|id| {
                rules.iter().any(|rule| {
                    &rule.id == id
                        && matches!(
                            (decision, rule.effect),
                            (Decision::Allow, PermissionEffect::Allow)
                                | (Decision::Deny, PermissionEffect::Deny)
                        )
                })
            })
            .collect();
        rule_ids.sort();
        let rule_id = rule_ids.into_iter().next();
        Ok(match (decision, rule_id) {
            (Decision::Allow, Some(rule_id)) => RuleSettlement::Allow { rule_id },
            (Decision::Deny, Some(rule_id)) => RuleSettlement::Deny { rule_id },
            // No stored policy took part. Cedar's default deny is "ask".
            _ => RuleSettlement::Ask,
        })
    }

    fn may(
        &self,
        who: RulePrincipal,
        change: RuleChange,
    ) -> Result<bool, PermissionRuleUnavailable> {
        let bundle = BUNDLE.as_ref().map_err(|_| PermissionRuleUnavailable)?;
        let action = match change {
            RuleChange::Grant => "rule.grant",
            RuleChange::Revoke => "rule.revoke",
            RuleChange::List => "rule.list",
        };
        let response = authorized(
            bundle,
            &bundle.policies,
            principal(who)?,
            action,
            tool_entity("rule", "claude")?,
            Context::empty(),
        )?;
        Ok(response.decision() == Decision::Allow)
    }
}

fn authorized(
    bundle: &Bundle,
    policies: &PolicySet,
    principal: Entity,
    action: &str,
    resource: Entity,
    context: Context,
) -> Result<cedar_policy::Response, PermissionRuleUnavailable> {
    let schema = &bundle.schema;
    let request = Request::new(
        principal.uid(),
        uid(ACTION, action)?,
        resource.uid(),
        context,
        Some(schema),
    )
    .map_err(|_| PermissionRuleUnavailable)?;
    let entities = Entities::from_entities([principal, resource], Some(schema))
        .map_err(|_| PermissionRuleUnavailable)?;
    let response = Authorizer::new().is_authorized(&request, policies, &entities);
    if response.diagnostics().errors().next().is_some() {
        return Err(PermissionRuleUnavailable);
    }
    Ok(response)
}

fn rule_policy(rule: &StoredPermissionRule) -> Result<Policy, PermissionRuleUnavailable> {
    let id = format!("{PREFIX}{}", rule.id);
    let effect = match rule.effect {
        PermissionEffect::Allow => "permit",
        PermissionEffect::Deny => "forbid",
    };
    let policy = json!({
        "annotations": { "id": id },
        "effect": effect,
        "principal": { "op": "All" },
        "action": { "op": "==", "entity": { "type": ACTION, "id": "rule.settle" } },
        "resource": { "op": "is", "entity_type": TOOL },
        "conditions": [{
            "kind": "when",
            "body": and(
                equals(attr("resource", "name"), value(&rule.tool)),
                and(
                    equals(attr("resource", "source"), value(&rule.source)),
                    equals(attr("context", "workspace"), value(&rule.workspace)),
                ),
            )
        }]
    });
    Policy::from_json(Some(PolicyId::new(&id)), policy).map_err(|_| PermissionRuleUnavailable)
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

fn value(text: &str) -> Value {
    json!({ "Value": text })
}

fn context(workspace: &str) -> Result<Context, PermissionRuleUnavailable> {
    Context::from_pairs([(
        "workspace".to_owned(),
        RestrictedExpression::new_string(workspace.to_owned()),
    )])
    .map_err(|_| PermissionRuleUnavailable)
}

fn principal(who: RulePrincipal) -> Result<Entity, PermissionRuleUnavailable> {
    let (kind, id) = match who {
        RulePrincipal::Owner => (OWNER, "owner"),
        RulePrincipal::Grant => (GRANT, "grant"),
    };
    entity(kind, id, [])
}

fn tool_entity(name: &str, source: &str) -> Result<Entity, PermissionRuleUnavailable> {
    entity(
        TOOL,
        &format!("{source}/{name}"),
        [
            ("name", RestrictedExpression::new_string(name.to_owned())),
            (
                "source",
                RestrictedExpression::new_string(source.to_owned()),
            ),
        ],
    )
}

fn entity(
    entity_type: &str,
    id: &str,
    attributes: impl IntoIterator<Item = (&'static str, RestrictedExpression)>,
) -> Result<Entity, PermissionRuleUnavailable> {
    Entity::new(
        uid(entity_type, id)?,
        attributes
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect::<HashMap<_, _>>(),
        HashSet::new(),
    )
    .map_err(|_| PermissionRuleUnavailable)
}

fn uid(entity_type: &str, id: &str) -> Result<EntityUid, PermissionRuleUnavailable> {
    let entity_type =
        EntityTypeName::from_str(entity_type).map_err(|_| PermissionRuleUnavailable)?;
    Ok(EntityUid::from_type_name_and_id(
        entity_type,
        EntityId::new(id),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nessa_sdk::application::agent_execution::permissions::ActionContext;

    fn granted() -> ActionContext {
        ActionContext::new("owner", "surface", "request").unwrap()
    }

    fn rule(
        id: &str,
        effect: PermissionEffect,
        tool: &str,
        workspace: &str,
    ) -> StoredPermissionRule {
        StoredPermissionRule {
            id: id.to_owned(),
            revision: "3".to_owned(),
            workspace: workspace.to_owned(),
            source: "claude".to_owned(),
            tool: tool.to_owned(),
            effect,
            granted_by: granted(),
        }
    }

    #[test]
    fn an_allow_rule_answers_only_that_tool_in_that_workspace() {
        let evaluator = CedarPermissionRuleEvaluator;
        let rules = [rule("rule-a", PermissionEffect::Allow, "Bash", "/work")];
        assert_eq!(
            evaluator
                .settle(RulePrincipal::Owner, "/work", "claude", "Bash", &rules)
                .unwrap(),
            RuleSettlement::Allow {
                rule_id: "rule-a".to_owned()
            }
        );
        assert_eq!(
            evaluator
                .settle(RulePrincipal::Owner, "/other", "claude", "Bash", &rules)
                .unwrap(),
            RuleSettlement::Ask
        );
        assert_eq!(
            evaluator
                .settle(RulePrincipal::Grant, "/work", "codex", "Bash", &rules)
                .unwrap(),
            RuleSettlement::Ask
        );
        assert_eq!(
            evaluator
                .settle(RulePrincipal::Owner, "/work", "claude", "Read", &rules)
                .unwrap(),
            RuleSettlement::Ask
        );
    }

    #[test]
    fn a_deny_rule_denies_and_a_miss_asks() {
        let evaluator = CedarPermissionRuleEvaluator;
        let rules = [rule("rule-d", PermissionEffect::Deny, "Bash", "/work")];
        assert_eq!(
            evaluator
                .settle(RulePrincipal::Grant, "/work", "claude", "Bash", &rules)
                .unwrap(),
            RuleSettlement::Deny {
                rule_id: "rule-d".to_owned()
            }
        );
        assert_eq!(
            evaluator
                .settle(RulePrincipal::Owner, "/work", "claude", "Bash", &[])
                .unwrap(),
            RuleSettlement::Ask
        );
    }

    #[test]
    fn only_the_owner_may_change_rules() {
        let evaluator = CedarPermissionRuleEvaluator;
        for change in [RuleChange::Grant, RuleChange::Revoke, RuleChange::List] {
            assert!(evaluator.may(RulePrincipal::Owner, change).unwrap());
            assert!(!evaluator.may(RulePrincipal::Grant, change).unwrap());
        }
    }
}
