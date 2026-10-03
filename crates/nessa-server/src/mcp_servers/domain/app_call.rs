//! Whether an MCP App may make the call it asks for (#348): the policy rows,
//! as pure rules over what the conversation's view says of the app and what
//! its server's session listed.
use nessa_sdk::domain::mcp_apps::ListedTool;

/// The most an app's tool arguments may be, encoded: the most a review
/// of the call shows (`ConversationPermission.argumentsJson`).
pub const MAX_APP_ARGUMENTS_BYTES: usize = 32 * 1024;
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

/// Whether the app `app` may call `tool` on `server`, given the tool as the
/// conversation's own session last listed it (`None` when it did not) and
/// the arguments' encoded size.
pub fn admit_tool_call(
    app: Option<&AppFacts>,
    server: &str,
    tool: Option<&ListedTool>,
    arguments_bytes: usize,
) -> Result<AppCallAdmission, AppRefusal> {
    admit_app(app, server)?;
    let tool = tool.ok_or(AppRefusal::ToolNotForApp)?;
    // With a UI or without one: a tool that says nothing is for both.
    if !tool.ui().visibility().app() {
        return Err(AppRefusal::ToolNotForApp);
    }
    if arguments_bytes > MAX_APP_ARGUMENTS_BYTES {
        return Err(AppRefusal::RequestTooLarge);
    }
    Ok(if tool.hints().destructive() {
        AppCallAdmission::Approve
    } else {
        AppCallAdmission::Send
    })
}

/// Whether `app` is an app at all, asking of its own `server`: all a
/// resource read needs, and the first of what a tool call does.
pub fn admit_app(app: Option<&AppFacts>, server: &str) -> Result<(), AppRefusal> {
    let app = app.filter(|app| app.has_ui).ok_or(AppRefusal::AppUnknown)?;
    if app.server != server {
        return Err(AppRefusal::ServerMismatch);
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/mcp_servers/app_call.rs"]
mod tests;
