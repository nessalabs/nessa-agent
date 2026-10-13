// Which tools a turn may call (slice J4 of #707). A permit leaves the call to
// the conversation's approval mode: a person is still asked where it asks.
// A tool no policy permits is refused, so a name not listed below, such as
// an MCP server's tool, is refused by every preset that lists names.
//
// `resource.source` is who named the tool: "claude", "codex", "opencode" or
// "gateway". A name is read only next to its source.

// The owner's own turns call any tool.
@id("tool.owner")
permit (
  principal is Nessa::Owner,
  action == Nessa::Action::"tool.call",
  resource is Nessa::Tool
);

// Tools that only read this machine's files, for read_only_tools and
// no_shell. Codex names the kind of action when it names no tool of its own;
// its "search" kind is also its web search, so it is not listed here.
@id("tool.reads")
permit (
  principal is Nessa::Grant,
  action == Nessa::Action::"tool.call",
  resource is Nessa::Tool
)
when {
  ["read_only_tools", "no_shell"].contains(principal.toolPreset) &&
  ((resource.source == "claude" && ["Read", "Glob", "Grep", "LS"].contains(resource.name)) ||
   (resource.source == "codex" && resource.name == "read") ||
   (resource.source == "gateway" && resource.name == "environments_list"))
};

// Tools that change files or reach the network but run no command, for
// no_shell. A tool that can run a command is never listed here.
@id("tool.edits")
permit (
  principal is Nessa::Grant,
  action == Nessa::Action::"tool.call",
  resource is Nessa::Tool
)
when {
  principal.toolPreset == "no_shell" &&
  ((resource.source == "claude" &&
    ["Write", "Edit", "MultiEdit", "NotebookEdit", "TodoWrite", "WebFetch", "WebSearch"]
      .contains(resource.name)) ||
   (resource.source == "codex" &&
    ["edit", "delete", "move", "search", "fetch", "think"].contains(resource.name)) ||
   (resource.source == "gateway" &&
    ["artifacts_publish", "artifacts_fetch"].contains(resource.name)))
};

// Every tool, each one asked for. The gateway admits a Drive turn only in the
// Ask approval mode (slice J4 part 3), so the person is asked for every call
// this permits.
@id("tool.preset.ask_everything")
permit (
  principal is Nessa::Grant,
  action == Nessa::Action::"tool.call",
  resource is Nessa::Tool
)
when { principal.toolPreset == "ask_everything" };

// Every tool, asked as the conversation's approval mode asks.
@id("tool.preset.all")
permit (
  principal is Nessa::Grant,
  action == Nessa::Action::"tool.call",
  resource is Nessa::Tool
)
when { principal.toolPreset == "all" };
