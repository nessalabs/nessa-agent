//! The command policy port, answered by the bundle's `commands.cedar`.

use cedar_policy::{Authorizer, Context, Entity, RestrictedExpression};

use crate::conversation::application::{
    CommandInitiator, CommandPolicyEvaluator, CommandQuestion, ToolDecision, ToolPolicyUnavailable,
};
use crate::conversation::domain::{program_name, CommandPolicy};

use super::bundle::Bundle;
use super::request::{agent, decide, entity, grant, owner, strings, uid, Ask};

const GATEWAY: &str = "Nessa::Gateway";
const HOST: &str = "Nessa::Host";
const COMMAND_HOST: &str = "command.host";
const COMMAND_RUN: &str = "command.run";
/// The one gateway: this one.
const THIS_GATEWAY: &str = "gateway";

/// The command policy port's Cedar adapter, over one configured
/// [`CommandPolicy`]. Build it once, in composition, whenever the policy is.
pub struct CedarCommandPolicyEvaluator {
    bundle: Bundle,
    authorizer: Authorizer,
    /// The gateway with what it is configured with, built once.
    gateway: Entity,
}

impl CedarCommandPolicyEvaluator {
    /// Load and strictly validate the checked-in bundle, and read `policy`
    /// as the gateway's facts.
    pub fn new(policy: &CommandPolicy) -> Result<Self, ToolPolicyUnavailable> {
        Self::with_bundle(Bundle::load()?, policy)
    }

    fn with_bundle(bundle: Bundle, policy: &CommandPolicy) -> Result<Self, ToolPolicyUnavailable> {
        let gateway = entity(
            GATEWAY,
            THIS_GATEWAY,
            [
                ("commandHosts", strings(policy.hosts())),
                (
                    "allowListed",
                    RestrictedExpression::new_bool(policy.allow().is_some()),
                ),
                (
                    "allowPrograms",
                    strings(policy.allow().into_iter().flatten()),
                ),
                ("denyPrograms", strings(policy.deny())),
            ],
        )?;
        Ok(Self {
            bundle,
            authorizer: Authorizer::new(),
            gateway,
        })
    }
}

impl CommandPolicyEvaluator for CedarCommandPolicyEvaluator {
    fn evaluate(
        &self,
        initiator: CommandInitiator<'_>,
        question: CommandQuestion<'_>,
    ) -> Result<ToolDecision, ToolPolicyUnavailable> {
        let principal = match initiator {
            CommandInitiator::Owner => owner()?,
            CommandInitiator::Agent => agent()?,
            CommandInitiator::Grant { grant: id, policy } => {
                grant(id, policy, Some(policy.command_hosts().names()))?
            }
        };
        let (host, action, context) = match question {
            CommandQuestion::Host { host } => (host, COMMAND_HOST, Context::empty()),
            CommandQuestion::Run { host, program } => {
                let context = Context::from_pairs([
                    (
                        "program".to_owned(),
                        RestrictedExpression::new_string(program.to_owned()),
                    ),
                    (
                        "programName".to_owned(),
                        RestrictedExpression::new_string(program_name(program).to_owned()),
                    ),
                ])
                .map_err(|_| ToolPolicyUnavailable)?;
                (host, COMMAND_RUN, context)
            }
        };
        let host = entity(
            HOST,
            host,
            [
                ("name", RestrictedExpression::new_string(host.to_owned())),
                (
                    "gateway",
                    RestrictedExpression::new_entity_uid(uid(GATEWAY, THIS_GATEWAY)?),
                ),
            ],
        )?;
        let ask = Ask {
            principal,
            action,
            resource: host,
            others: vec![self.gateway.clone()],
            context,
        };
        decide(&self.authorizer, &self.bundle, self.bundle.policies(), ask)
    }
}

#[cfg(test)]
#[path = "../../../../../tests/conversation/command_policy_cedar.rs"]
mod tests;
