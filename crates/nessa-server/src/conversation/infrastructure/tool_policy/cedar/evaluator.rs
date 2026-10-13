//! The tool policy port, answered by the bundle.

use cedar_policy::{Authorizer, Context, Entity, PolicySet, RestrictedExpression};

use crate::conversation::application::{
    ToolDecision, ToolInitiator, ToolPolicyEvaluator, ToolPolicyUnavailable,
};
use crate::conversation::domain::{ToolCall, ToolPolicy, ToolSource};

use super::bundle::Bundle;
use super::grant_rules::grant_policies;
use super::request::{decide, entity, grant, owner, Ask};

pub(super) const TOOL: &str = "Nessa::Tool";
pub(super) const TOOL_CALL: &str = "tool.call";

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
        let tool = entity(
            TOOL,
            call.name(),
            [
                (
                    "name",
                    RestrictedExpression::new_string(call.name().to_owned()),
                ),
                (
                    "source",
                    RestrictedExpression::new_string(source(call.source()).to_owned()),
                ),
            ],
        )?;
        let ask = Ask {
            principal,
            action: TOOL_CALL,
            resource: tool,
            others: Vec::new(),
            context: Context::empty(),
        };
        decide(&self.authorizer, &self.bundle, policies, ask)
    }
}

impl ToolPolicyEvaluator for CedarToolPolicyEvaluator {
    fn evaluate(
        &self,
        initiator: ToolInitiator<'_>,
        call: ToolCall<'_>,
    ) -> Result<ToolDecision, ToolPolicyUnavailable> {
        match initiator {
            ToolInitiator::Owner => self.decide(owner()?, self.bundle.policies(), call),
            ToolInitiator::Grant { grant: id, policy } => {
                let policies = match policy {
                    ToolPolicy::Preset(_) => None,
                    ToolPolicy::Rules(rules) => Some(self.bundle.with(grant_policies(id, rules)?)?),
                };
                let principal = grant(id, policy, None)?;
                let policies = policies.as_ref().unwrap_or(self.bundle.policies());
                self.decide(principal, policies, call)
            }
        }
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
