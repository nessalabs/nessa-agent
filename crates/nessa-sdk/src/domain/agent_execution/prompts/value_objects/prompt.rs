#![deny(missing_docs)]

use crate::domain::agent_execution::ExecutionError;

/// Nonblank instruction or user-message text. Preserve its exact whitespace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptText(Box<str>);
impl PromptText {
    /// Own `value` without trimming accepted whitespace. Returns
    /// [`ExecutionError::EmptyValue`] when all characters are whitespace or empty.
    /// Accepted text is compacted so retained bytes equal its UTF-8 length.
    /// This validates text presence, not token budgets or instruction safety.
    pub fn new(value: impl Into<String>) -> Result<Self, ExecutionError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ExecutionError::EmptyValue("prompt text"));
        }
        Ok(Self(value.into_boxed_str()))
    }
    /// Borrow the exact validated text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Who supplied a contribution. This describes provenance, not message role or authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptSourceKind {
    /// Instructions supplied by the embedding application.
    Core,
    /// Instructions contributed by an installed plugin.
    Plugin,
    /// Instructions contributed by an MCP integration.
    Mcp,
    /// Instructions contributed by a selected skill.
    Skill,
    /// User-authored system customization, not a conversational user message.
    User,
}

/// A named contributor, such as the core system prompt, a plugin, or an MCP server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptSource {
    kind: PromptSourceKind,
    name: Box<str>,
}
impl PromptSource {
    /// Associate contributor `kind` with an exact human-readable `name`. Returns
    /// [`ExecutionError::EmptyValue`] for a blank name; does not authenticate it.
    /// Accepted names retain exact text without caller allocation spare capacity.
    pub fn new(kind: PromptSourceKind, name: impl Into<String>) -> Result<Self, ExecutionError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(ExecutionError::EmptyValue("prompt source name"));
        }
        Ok(Self {
            kind,
            name: name.into_boxed_str(),
        })
    }
    /// Provenance category, which does not assign instruction authority.
    pub fn kind(&self) -> PromptSourceKind {
        self.kind
    }
    /// Original contributor name for inspection and attribution.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// One append and its source. Empty text and whitespace separators retain attribution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptContribution {
    source: PromptSource,
    text: Box<str>,
}
impl PromptContribution {
    /// Own `text` attributed to `source`, preserving empty text and separators.
    /// Contributions concatenate without inserted separators when assembled.
    /// Immutable text storage discards caller allocation spare capacity.
    pub fn new(source: PromptSource, text: impl Into<String>) -> Self {
        Self {
            source,
            text: text.into().into_boxed_str(),
        }
    }
    /// Contributor attributed to this exact text fragment.
    pub fn source(&self) -> &PromptSource {
        &self.source
    }
    /// Borrow the exact fragment, including whitespace and empty text.
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// Borrowed provenance and exact text of one assembled contribution.
/// The text points into its owning SystemPrompt; inspection makes no payload copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PromptContributionView<'a> {
    source: &'a PromptSource,
    text: &'a str,
}
impl<'a> PromptContributionView<'a> {
    /// Borrow the original source, without assigning instruction authority.
    pub fn source(&self) -> &'a PromptSource {
        self.source
    }
    /// Borrow the exact fragment, including empty text and whitespace separators.
    pub fn text(&self) -> &'a str {
        self.text
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PromptContributionRange {
    source: PromptSource,
    start: usize,
    end: usize,
}

/// Attributed system instructions, configured separately from user messages.
/// Source kinds describe contributors, not message roles or instruction priority.
/// One immutable text allocation backs the complete prompt and all contribution
/// views; only sources and byte ranges are retained per contribution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemPrompt {
    text: PromptText,
    contributions: Box<[PromptContributionRange]>,
}
impl SystemPrompt {
    /// Assemble exact bytes in `contributions` order; source metadata never enters
    /// the text and no separators are inserted. Consumes each owned fragment and
    /// retains only its source and UTF-8 range in the single assembled backing.
    /// Empty appends retain their source and zero-length range. Unused collection
    /// capacity is discarded. Returns [`ExecutionError::EmptyValue`] if all text
    /// is blank, including an empty contribution list, before allocating joined text.
    /// Does not resolve files, impose a byte cap, or validate instruction safety.
    ///
    /// Construction temporarily holds input fragments alongside the final text
    /// buffer (up to twice the text bytes, plus metadata); each fragment is dropped
    /// after copying. The completed prompt retains text once. Cloning explicitly
    /// creates a separate backing and preserves the same contribution boundaries.
    ///
    /// # Examples
    /// ```
    /// use nessa_sdk::domain::agent_execution::prompts::{
    ///     PromptContribution, PromptSource, PromptSourceKind, SystemPrompt,
    /// };
    /// let source = PromptSource::new(PromptSourceKind::Core, "application")?;
    /// let prompt = SystemPrompt::new(vec![PromptContribution::new(source, "Be concise.")])?;
    /// assert_eq!(prompt.text().as_str(), "Be concise.");
    /// assert_eq!(prompt.contributions().next().unwrap().text(), "Be concise.");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new(contributions: Vec<PromptContribution>) -> Result<Self, ExecutionError> {
        if !contributions
            .iter()
            .any(|part| !part.text().trim().is_empty())
        {
            return Err(ExecutionError::EmptyValue("prompt text"));
        }
        let bytes = contributions.iter().map(|part| part.text.len()).sum();
        let mut text = String::with_capacity(bytes);
        let mut ranges = Vec::with_capacity(contributions.len());
        for contribution in contributions {
            let start = text.len();
            text.push_str(&contribution.text);
            ranges.push(PromptContributionRange {
                source: contribution.source,
                start,
                end: text.len(),
            });
        }
        Ok(Self {
            text: PromptText(text.into_boxed_str()),
            contributions: ranges.into_boxed_slice(),
        })
    }
    /// Borrow the complete validated system instruction text without rendering it.
    pub fn text(&self) -> &PromptText {
        &self.text
    }
    /// Retained allocation payload bytes: the single text backing, compact
    /// contribution slots, and source-name text. Excludes the inline SystemPrompt
    /// value and allocator bookkeeping; this is not a process-memory ceiling.
    /// No instruction text is copied or rendered to compute this measurement.
    pub fn payload_bytes(&self) -> usize {
        self.contributions
            .iter()
            .fold(self.text.as_str().len(), |bytes, part| {
                bytes
                    .saturating_add(std::mem::size_of::<PromptContributionRange>())
                    .saturating_add(part.source.name().len())
            })
    }
    /// Iterate over contributions in assembly order, borrowing each source and its
    /// exact range from this prompt. Empty contributions remain visible. The
    /// iterator reports its remaining length and supports reverse iteration; it
    /// allocates no collection and copies no source or instruction text.
    pub fn contributions(
        &self,
    ) -> impl ExactSizeIterator<Item = PromptContributionView<'_>> + DoubleEndedIterator {
        self.contributions
            .iter()
            .map(|part| PromptContributionView {
                source: &part.source,
                text: &self.text.as_str()[part.start..part.end],
            })
    }
}

#[cfg(test)]
#[path = "../../../../../tests/domain/agent_execution/prompt_storage.rs"]
mod storage_tests;
