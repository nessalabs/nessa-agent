#![deny(missing_docs)]

use crate::domain::agent_execution::{
    prompts::{PromptContribution, PromptSource, SystemPrompt},
    ExecutionError,
};

/// Builds system instructions in insertion order, preserving attribution and exact text.
/// Callers resolve files, templates, or other sources before adding their text.
///
/// ```
/// use nessa_sdk::domain::agent_execution::{
///     prompts::{PromptSource, PromptSourceKind, SystemPromptBuilder},
///     ExecutionError,
/// };
///
/// let source = PromptSource::new(PromptSourceKind::Core, "app/system")?;
/// let prompt = SystemPromptBuilder::new()
///     .text(source, "Explain changes clearly.")
///     .build()?;
/// assert_eq!(prompt.text().as_str(), "Explain changes clearly.");
/// # Ok::<(), ExecutionError>(())
/// ```
#[derive(Default)]
pub struct SystemPromptBuilder {
    contributions: Vec<PromptContribution>,
}

impl SystemPromptBuilder {
    /// Create an empty builder without resolving files or performing provider effects.
    pub fn new() -> Self {
        Self::default()
    }
    /// Append `text` with its validated `source` attribution in insertion order.
    /// Consumes and returns the builder, owning the supplied text exactly, including
    /// whitespace. No separators are inserted; validation occurs in `build`.
    pub fn text(mut self, source: PromptSource, text: impl Into<String>) -> Self {
        self.contributions
            .push(PromptContribution::new(source, text));
        self
    }
    /// Consume the contributions and construct the ordered system prompt.
    /// Returns `EmptyValue` when the assembled text is blank. This pure operation
    /// preserves contribution attribution in ranges over one assembled text backing
    /// and does not configure a provider. Construction temporarily overlaps the
    /// owned input fragments and assembled buffer; see SystemPrompt::new.
    pub fn build(self) -> Result<SystemPrompt, ExecutionError> {
        SystemPrompt::new(self.contributions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agent_execution::prompts::PromptSourceKind;

    #[test]
    fn builds_a_system_prompt_with_inspectable_core_plugin_mcp_and_skill_layers() {
        const SYSTEM_PROMPT: &str = "You are Nessa, a helpful assistant.";
        const PLUGIN_PROMPT: &str = "\n\nWhen reviewing code, explain correctness issues first.";
        const MCP_PROMPT: &str = "\n\nUse the workspace server to look up project files.";
        const SKILL_PROMPT: &str = "\n\nFor Rust changes, keep domain rules independent of I/O.";

        // The host resolves these texts, then appends each layer with its contributor.
        let prompt = SystemPromptBuilder::new()
            .text(
                PromptSource::new(PromptSourceKind::Core, "nessa/system").unwrap(),
                SYSTEM_PROMPT,
            )
            .text(
                PromptSource::new(PromptSourceKind::Plugin, "code-review").unwrap(),
                PLUGIN_PROMPT,
            )
            .text(
                PromptSource::new(PromptSourceKind::Mcp, "workspace").unwrap(),
                MCP_PROMPT,
            )
            .text(
                PromptSource::new(PromptSourceKind::Skill, "rust").unwrap(),
                SKILL_PROMPT,
            )
            .build()
            .unwrap();

        // The assembled prompt contains only the supplied text, in append order.
        assert_eq!(
            prompt.text().as_str(),
            "You are Nessa, a helpful assistant.\n\n\
             When reviewing code, explain correctness issues first.\n\n\
             Use the workspace server to look up project files.\n\n\
             For Rust changes, keep domain rules independent of I/O."
        );

        // A future UI can inspect who contributed each section and its exact text.
        let contributions: Vec<_> = prompt
            .contributions()
            .map(|part| (part.source().kind(), part.source().name(), part.text()))
            .collect();
        assert_eq!(
            contributions,
            vec![
                (PromptSourceKind::Core, "nessa/system", SYSTEM_PROMPT),
                (PromptSourceKind::Plugin, "code-review", PLUGIN_PROMPT),
                (PromptSourceKind::Mcp, "workspace", MCP_PROMPT),
                (PromptSourceKind::Skill, "rust", SKILL_PROMPT),
            ]
        );
    }
}
