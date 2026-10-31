//! The question an MCP App's call answers before anything reaches its
//! server: may this app reach this server, and call this tool on it? (#348,
//! ADR 803 step 4.)
//!
//! Answered by Cedar (`infrastructure::tool_policy::cedar`, `apps.cedar`)
//! from what is true: the server whose tool call drew the app, the server it
//! names, and the tool as that server's session listed it. Nothing here
//! compares servers or reads a tool's visibility.
//!
//! ```text
//! app_calls ──▶ known_app (Rust: is it an app at all?)
//!           ──▶ admit_app_question(Reach)     ──▶ Cedar: app.own_server / app.other_server
//!           ──▶ listed_tool (Rust: did its session list the tool?)
//!           ──▶ admit_app_question(CallTool)  ──▶ Cedar: app.visible_tool / app.other_server_tool
//!           ──▶ admit_listed_call (Rust: argument size; destructive asks a person)
//! ```
//!
//! Each refusal is one of the protocol's existing codes: another server is
//! `mcp_server_mismatch`, a tool no policy lets the app call is
//! `mcp_tool_not_for_app`. A policy that could not be evaluated refuses as
//! the question it was asked would have: a server it could not confirm as
//! the app's own, or a tool it could not confirm the app may call.

use crate::mcp_servers::domain::AppRefusal;

use super::{ToolDecision, ToolPolicyUnavailable};

/// The id of the policy that refuses another server's tool.
const OTHER_SERVER_TOOL: &str = "app.other_server_tool";

/// What an app asks of the gateway.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppQuestion<'a> {
    /// May the app reach `server` at all? All a resource read, a message or
    /// a model context needs, and the first of what a tool call does.
    Reach {
        /// The server the app names.
        server: &'a str,
    },
    /// May the app call `tool` on `server`, as that server's session last
    /// listed it?
    CallTool {
        /// The server the tool was listed by.
        server: &'a str,
        /// The tool's name.
        tool: &'a str,
        /// Whether the tool's declared visibility includes apps: its
        /// `_meta.ui.visibility` names `app`, or it says nothing.
        visible_to_app: bool,
    },
}

/// What decides whether an app's message may go without asking the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnreviewedMessage<'a> {
    /// The server the app names.
    pub server: &'a str,
    /// The conversation's approval mode, as the wire names it.
    pub approval_mode: &'a str,
    /// Whether the person chose to always allow this app's messages.
    pub trusted_app: bool,
}

/// Answers whether an app may reach a server, or call a tool on it.
pub trait AppCallPolicyEvaluator: Send + Sync {
    /// The decision for `question`, asked by the app drawn by a tool call to
    /// `app_server`. The policy ids in a refusal name the rule that forbade
    /// it; none means no rule allowed it.
    fn evaluate(
        &self,
        app_server: &str,
        question: AppQuestion<'_>,
    ) -> Result<ToolDecision, ToolPolicyUnavailable>;

    /// Whether the app drawn by `app_server` may send `message` as the
    /// person's turn without the person reviewing it first.
    fn message_unreviewed(
        &self,
        app_server: &str,
        message: UnreviewedMessage<'_>,
    ) -> Result<ToolDecision, ToolPolicyUnavailable>;
}

/// Whether the app's message goes without a review. A policy that cannot be
/// evaluated, or that allows nothing, asks the person.
pub fn message_skips_review(
    evaluator: &dyn AppCallPolicyEvaluator,
    app_server: &str,
    message: UnreviewedMessage<'_>,
) -> bool {
    match evaluator.message_unreviewed(app_server, message) {
        Ok(ToolDecision::Allow { rules }) => {
            tracing::info!(app_server, ?rules, "MCP App message sent without a review");
            true
        }
        Ok(ToolDecision::Refuse { .. }) => false,
        Err(ToolPolicyUnavailable) => {
            tracing::error!(app_server, "MCP App message policy unavailable; asking");
            false
        }
    }
}

/// Whether the app drawn by `app_server` may do what `question` asks, as
/// the refusal the protocol answers with. `Ok` carries the ids of the
/// policies that allowed it; a refusal is logged with the ids that made it.
pub fn admit_app_question(
    evaluator: &dyn AppCallPolicyEvaluator,
    app_server: &str,
    question: AppQuestion<'_>,
) -> Result<Vec<String>, AppRefusal> {
    match evaluator.evaluate(app_server, question) {
        Ok(ToolDecision::Allow { rules }) => Ok(rules),
        Ok(ToolDecision::Refuse { rules }) => {
            tracing::info!(
                app_server,
                ?question,
                ?rules,
                "MCP App call refused by policy"
            );
            Err(match question {
                AppQuestion::Reach { .. } => AppRefusal::ServerMismatch,
                AppQuestion::CallTool { .. } if rules.iter().any(|id| id == OTHER_SERVER_TOOL) => {
                    AppRefusal::ServerMismatch
                }
                AppQuestion::CallTool { .. } => AppRefusal::ToolNotForApp,
            })
        }
        Err(ToolPolicyUnavailable) => {
            tracing::error!(
                app_server,
                ?question,
                "MCP App call policy unavailable; refused"
            );
            Err(match question {
                AppQuestion::Reach { .. } => AppRefusal::ServerMismatch,
                AppQuestion::CallTool { .. } => AppRefusal::ToolNotForApp,
            })
        }
    }
}
