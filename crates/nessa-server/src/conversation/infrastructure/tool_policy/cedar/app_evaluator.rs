//! The MCP App call policy port, answered by the bundle's `apps.cedar`.

use cedar_policy::{Authorizer, Context, Entity, RestrictedExpression};

use crate::conversation::application::{
    AppCallPolicyEvaluator, AppQuestion, ToolDecision, ToolPolicyUnavailable, UnreviewedMessage,
};

use super::bundle::Bundle;
use super::request::{decide, entity, uid, Ask};

const APP: &str = "Nessa::App";
const MCP_SERVER: &str = "Nessa::McpServer";
const MCP_TOOL: &str = "Nessa::McpTool";
const APP_REACH: &str = "app.reach";
const APP_CALL_TOOL: &str = "app.callTool";
const APP_SEND_UNREVIEWED: &str = "app.sendUnreviewed";

/// The MCP App call policy port's Cedar adapter. Build it once, in
/// composition.
pub struct CedarAppCallPolicyEvaluator {
    bundle: Bundle,
    authorizer: Authorizer,
}

impl CedarAppCallPolicyEvaluator {
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
}

impl AppCallPolicyEvaluator for CedarAppCallPolicyEvaluator {
    fn evaluate(
        &self,
        app_server: &str,
        question: AppQuestion<'_>,
    ) -> Result<ToolDecision, ToolPolicyUnavailable> {
        let own = server(app_server)?;
        let app = entity(
            APP,
            app_server,
            [("server", RestrictedExpression::new_entity_uid(own.uid()))],
        )?;
        let (action, resource, asked) = match question {
            AppQuestion::Reach { server: asked } => {
                let asked = server(asked)?;
                (APP_REACH, asked.clone(), asked)
            }
            AppQuestion::CallTool {
                server: asked,
                tool,
                visible_to_app,
            } => {
                let asked = server(asked)?;
                let tool = entity(
                    MCP_TOOL,
                    tool,
                    [
                        ("name", RestrictedExpression::new_string(tool.to_owned())),
                        ("server", RestrictedExpression::new_entity_uid(asked.uid())),
                        (
                            "visibleToApp",
                            RestrictedExpression::new_bool(visible_to_app),
                        ),
                    ],
                )?;
                (APP_CALL_TOOL, tool, asked)
            }
        };
        // Each server once: the app's own, and the one asked of when it is
        // another, unless it is the resource itself.
        let mut others = Vec::new();
        for server in [own, asked] {
            if server.uid() != resource.uid()
                && !others.iter().any(|o: &Entity| o.uid() == server.uid())
            {
                others.push(server);
            }
        }
        let ask = Ask {
            principal: app,
            action,
            resource,
            others,
            context: Context::empty(),
        };
        decide(&self.authorizer, &self.bundle, self.bundle.policies(), ask)
    }

    fn message_unreviewed(
        &self,
        app_server: &str,
        message: UnreviewedMessage<'_>,
    ) -> Result<ToolDecision, ToolPolicyUnavailable> {
        let own = server(app_server)?;
        let app = entity(
            APP,
            app_server,
            [("server", RestrictedExpression::new_entity_uid(own.uid()))],
        )?;
        let asked = server(message.server)?;
        let others = if asked.uid() == own.uid() {
            Vec::new()
        } else {
            vec![own]
        };
        let context = Context::from_pairs([
            (
                "approvalMode".to_owned(),
                RestrictedExpression::new_string(message.approval_mode.to_owned()),
            ),
            (
                "trustedApp".to_owned(),
                RestrictedExpression::new_bool(message.trusted_app),
            ),
        ])
        .map_err(|_| ToolPolicyUnavailable)?;
        let ask = Ask {
            principal: app,
            action: APP_SEND_UNREVIEWED,
            resource: asked,
            others,
            context,
        };
        decide(&self.authorizer, &self.bundle, self.bundle.policies(), ask)
    }
}

/// An MCP server, by its configured name.
fn server(name: &str) -> Result<Entity, ToolPolicyUnavailable> {
    Ok(Entity::new_no_attrs(
        uid(MCP_SERVER, name)?,
        Default::default(),
    ))
}

#[cfg(test)]
#[path = "../../../../../tests/conversation/app_call_policy.rs"]
mod tests;
