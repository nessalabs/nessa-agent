//! A tool call a policy is asked about: the tool's name, and who named it.
//!
//! A name means something only next to the agent that gave it. Claude calls
//! its file reader `Read`; Codex names the kind of action, `read`; the gateway
//! runs its own tools, `run` among them. Neither is checked against a list
//! here: what a name allows is the tool policy's to say, so a name nothing
//! lists is a fact the policy sees, not an error.
//!
//! ```text
//! ToolCall ──source──▶ ToolSource (Claude | Codex | OpenCode | Gateway)
//!     └─────name────▶ the tool's name, exactly as that source gave it
//! ```

/// Who named a tool: the agent harness that asked to call it, or the gateway
/// that runs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolSource {
    /// A Claude Code harness, naming its tools (`Read`, `Write`, `mcp__…`).
    Claude,
    /// A Codex harness, naming its own tool or the kind of action (`read`).
    Codex,
    /// An OpenCode harness.
    OpenCode,
    /// The gateway's own environment tools (`run`, `artifacts_publish`).
    Gateway,
}

/// One tool call, as its source named it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolCall<'a> {
    source: ToolSource,
    name: &'a str,
}

impl<'a> ToolCall<'a> {
    /// The call `source` named `name`.
    pub fn new(source: ToolSource, name: &'a str) -> Self {
        Self { source, name }
    }

    /// Who named the tool.
    pub fn source(&self) -> ToolSource {
        self.source
    }

    /// The tool's name, exactly as its source gave it.
    pub fn name(&self) -> &'a str {
        self.name
    }
}
