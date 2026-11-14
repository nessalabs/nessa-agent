//! What of an MCP App's call (#348) is not policy, as pure rules over what
//! the conversation's view says of the app and what its server's session
//! listed: whether it is an app at all, whether the tool was listed, how
//! large its arguments are, and whether a person must review it. Which
//! server it may reach and which tools it may call there are Cedar's
//! (`conversation::application::AppCallPolicyEvaluator`, ADR 803 step 4).
use nessa_sdk::domain::mcp_apps::ListedTool;

/// The most an app's tool arguments may be, encoded. The SDK's
/// [`MAX_MCP_ARGUMENTS_BYTES`](nessa_sdk::domain::agent_execution::tools::MAX_MCP_ARGUMENTS_BYTES):
/// one bound for a review, an app's own call, and the arguments a
/// conversation view carries. Not a second limit.
pub const MAX_APP_ARGUMENTS_BYTES: usize =
    nessa_sdk::domain::agent_execution::tools::MAX_MCP_ARGUMENTS_BYTES;
/// The most a tool's answer to an app may be, encoded.
pub const MAX_APP_RESULT_BYTES: usize = 56 * 1024;

/// What the conversation's view says of the tool call an app names as
/// itself: the MCP server it went to, and whether its tool declared a UI.
/// `None` when the conversation has no such call, or it is not an MCP call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppFacts {
    pub server: String,
    pub has_ui: bool,
}

/// Why an app's call is refused before anything reaches its server. Each is
/// one of the protocol's `mcp_` codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppRefusal {
    /// The app is no MCP tool call with a UI in this conversation.
    AppUnknown,
    /// It names another server than the app's own.
    ServerMismatch,
    /// The tool is not listed, or not for an app (`visibility`).
    ToolNotForApp,
    /// Its arguments are past [`MAX_APP_ARGUMENTS_BYTES`].
    RequestTooLarge,
}

/// What an admitted tool call needs before it is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppCallAdmission {
    /// Nothing: it may be sent now.
    Send,
    /// The person's approval first: the tool is destructive, whatever the
    /// conversation's approval mode — the mode is trust in the agent, not
    /// in an app.
    Approve,
}

/// The app `app` names, if it is one at all: an MCP tool call in this
/// conversation that declared a UI. A lookup, asked before the policy is.
pub fn known_app(app: Option<&AppFacts>) -> Result<&AppFacts, AppRefusal> {
    app.filter(|app| app.has_ui).ok_or(AppRefusal::AppUnknown)
}

/// The tool as the conversation's own session last listed it; `None` when
/// it did not, which no app may call. A lookup, asked before the policy is.
pub fn listed_tool(tool: Option<&ListedTool>) -> Result<&ListedTool, AppRefusal> {
    tool.ok_or(AppRefusal::ToolNotForApp)
}

/// What a listed tool's call that the policy allowed needs before it is
/// sent, given its arguments' encoded size: capacity, then whether a person
/// must review it.
pub fn admit_listed_call(
    tool: &ListedTool,
    arguments_bytes: usize,
) -> Result<AppCallAdmission, AppRefusal> {
    if arguments_bytes > MAX_APP_ARGUMENTS_BYTES {
        return Err(AppRefusal::RequestTooLarge);
    }
    Ok(if tool.hints().destructive() {
        AppCallAdmission::Approve
    } else {
        AppCallAdmission::Send
    })
}

#[cfg(test)]
#[path = "../../../tests/mcp_servers/app_call.rs"]
mod tests;
