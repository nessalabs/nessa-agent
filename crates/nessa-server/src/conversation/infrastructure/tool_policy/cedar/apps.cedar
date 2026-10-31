// What an MCP App may ask of the gateway (#348, ADR 803 step 4). Whether the
// app is one at all, whether its server listed the tool, how large its
// arguments are, and whether a person must review a destructive tool are
// not policy, and are decided before or after these.
//
// `principal.server` is the server whose tool call drew the app.

// An app reaches its own server.
@id("app.own_server")
permit (
  principal is Nessa::App,
  action == Nessa::Action::"app.reach",
  resource is Nessa::McpServer
)
when { resource == principal.server };

// And never another one.
@id("app.other_server")
forbid (
  principal is Nessa::App,
  action == Nessa::Action::"app.reach",
  resource is Nessa::McpServer
)
when { resource != principal.server };

// An app calls a tool of its own server that is visible to apps: one whose
// `_meta.ui.visibility` names `app`, or says nothing, which is both.
@id("app.visible_tool")
permit (
  principal is Nessa::App,
  action == Nessa::Action::"app.callTool",
  resource is Nessa::McpTool
)
when { resource.server == principal.server && resource.visibleToApp };

// Never another server's tool, whatever it is.
@id("app.other_server_tool")
forbid (
  principal is Nessa::App,
  action == Nessa::Action::"app.callTool",
  resource is Nessa::McpTool
)
when { resource.server != principal.server };

// An app's message goes as the person's turn without asking them when the
// conversation runs with full access: its agent already acts without asking.
// `context.approvalMode` is the conversation's mode: `ask`, `auto` or `full`.
@id("app.message_full_access")
permit (
  principal is Nessa::App,
  action == Nessa::Action::"app.sendUnreviewed",
  resource is Nessa::McpServer
)
when { resource == principal.server && context.approvalMode == "full" };

// Or when the person chose "Always allow this app" for the app's server.
@id("app.message_trusted_app")
permit (
  principal is Nessa::App,
  action == Nessa::Action::"app.sendUnreviewed",
  resource is Nessa::McpServer
)
when { resource == principal.server && context.trustedApp };
