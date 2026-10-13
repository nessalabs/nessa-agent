//! The tool policy port, answered by the bundle.

use cedar_policy::{
    Authorizer, Context, Decision, Entities, Entity, EntityId, EntityTypeName, EntityUid,
    PolicySet, Request, RestrictedExpression,
};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use crate::conversation::application::{
    ToolDecision, ToolInitiator, ToolPolicyEvaluator, ToolPolicyUnavailable,
};
use crate::conversation::domain::{ToolCall, ToolPolicy, ToolPreset, ToolSource};

use super::bundle::Bundle;
use super::grant_rules::grant_policies;

pub(super) const OWNER: &str = "Nessa::Owner";
pub(super) const GRANT: &str = "Nessa::Grant";
pub(super) const TOOL: &str = "Nessa::Tool";
pub(super) const ACTION: &str = "Nessa::Action";
pub(super) const TOOL_CALL: &str = "tool.call";
/// A grant whose policy is its own rules names this as its preset, so no
/// checked-in preset policy applies to it.
const RULES: &str = "rules";

/// The tool policy port's Cedar adapter. Build it once, in composition.
pub struct CedarToolPolicyEvaluator {
    bundle: Bundle,
    authorizer: Authorizer,
}

impl CedarToolPolicyEvaluator {
    /// Load and strictly validate the checked-in bundle.
    pub fn new() -> Result<Self, ToolPolicyUnavailable> {
        Ok(Self::with_bundle(Bundle::load()?))
    }

    fn with_bundle(bundle: Bundle) -> Self {
        Self {
            bundle,
            authorizer: Authorizer::new(),
        }
    }

    fn decide(
        &self,
        principal: Entity,
        policies: &PolicySet,
        call: ToolCall<'_>,
    ) -> Result<ToolDecision, ToolPolicyUnavailable> {
        let schema = self.bundle.schema();
        let tool = Entity::new(
            uid(TOOL, call.name())?,
            HashMap::from([
                (
                    "name".to_owned(),
                    RestrictedExpression::new_string(call.name().to_owned()),
                ),
                (
                    "source".to_owned(),
                    RestrictedExpression::new_string(source(call.source()).to_owned()),
                ),
            ]),
            HashSet::new(),
        )
        .map_err(|_| ToolPolicyUnavailable)?;
        let request = Request::new(
            principal.uid(),
            uid(ACTION, TOOL_CALL)?,
            tool.uid(),
            Context::empty(),
            Some(schema),
        )
        .map_err(|_| ToolPolicyUnavailable)?;
        let entities = Entities::from_entities([principal, tool], Some(schema))
            .map_err(|_| ToolPolicyUnavailable)?;
        let response = self.authorizer.is_authorized(&request, policies, &entities);
        // Cedar skips a policy whose condition errors, a forbid included, so
        // any error refuses the whole decision rather than let it open.
        if response.diagnostics().errors().next().is_some() {
            return Err(ToolPolicyUnavailable);
        }
        let mut rules: Vec<String> = response
            .diagnostics()
            .reason()
            .map(ToString::to_string)
            .collect();
        rules.sort();
        Ok(match response.decision() {
            Decision::Allow => ToolDecision::Allow { rules },
            Decision::Deny => ToolDecision::Refuse { rules },
        })
    }
}

impl ToolPolicyEvaluator for CedarToolPolicyEvaluator {
    fn evaluate(
        &self,
        initiator: ToolInitiator<'_>,
        call: ToolCall<'_>,
    ) -> Result<ToolDecision, ToolPolicyUnavailable> {
        match initiator {
            ToolInitiator::Owner => {
                let owner = Entity::new_no_attrs(uid(OWNER, "owner")?, HashSet::new());
                self.decide(owner, self.bundle.policies(), call)
            }
            ToolInitiator::Grant { grant, policy } => {
                let (preset, policies) = match policy {
                    ToolPolicy::Preset(preset) => (preset_name(*preset), None),
                    ToolPolicy::Rules(rules) => (
                        RULES,
                        Some(self.bundle.with(grant_policies(grant, rules)?)?),
                    ),
                };
                let principal = Entity::new(
                    uid(GRANT, grant)?,
                    HashMap::from([(
                        "toolPreset".to_owned(),
                        RestrictedExpression::new_string(preset.to_owned()),
                    )]),
                    HashSet::new(),
                )
                .map_err(|_| ToolPolicyUnavailable)?;
                let policies = policies.as_ref().unwrap_or(self.bundle.policies());
                self.decide(principal, policies, call)
            }
        }
    }
}

fn uid(entity_type: &str, id: &str) -> Result<EntityUid, ToolPolicyUnavailable> {
    let entity_type = EntityTypeName::from_str(entity_type).map_err(|_| ToolPolicyUnavailable)?;
    Ok(EntityUid::from_type_name_and_id(
        entity_type,
        EntityId::new(id),
    ))
}

/// A preset's name, as `tools.cedar` compares it.
fn preset_name(preset: ToolPreset) -> &'static str {
    match preset {
        ToolPreset::ReadOnlyTools => "read_only_tools",
        ToolPreset::NoShell => "no_shell",
        ToolPreset::AskEverything => "ask_everything",
        ToolPreset::All => "all",
    }
}

/// Who named a tool, as `tools.cedar` compares it.
fn source(source: ToolSource) -> &'static str {
    match source {
        ToolSource::Claude => "claude",
        ToolSource::Codex => "codex",
        ToolSource::OpenCode => "opencode",
        ToolSource::Gateway => "gateway",
    }
}

#[cfg(test)]
#[path = "../../../../../tests/conversation/tool_policy.rs"]
mod tests;
