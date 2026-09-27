//! Product approval choices published by each provider binding.
//!
//! A choice is a description of one native preset. The binding owns its wire
//! spelling and model availability; callers may display these values but may
//! not infer approval behavior from the product ID alone.

/// Nessa's stable name for a native provider approval preset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalMode {
    /// Use the provider's ordinary approval requests.
    Ask,
    /// Use the provider's supported automatic review preset.
    Auto,
    /// Use the provider's most permissive preset, subject to fixed Nessa denials.
    Full,
}
impl ApprovalMode {
    /// The product wire spelling of this choice.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Auto => "auto",
            Self::Full => "full",
        }
    }
}

/// A binding-owned label and description for one selectable preset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApprovalModeChoice {
    /// Product choice, mapped to the provider's native mode by its binding.
    pub id: ApprovalMode,
    /// Short label for a model picker or conversation details.
    pub name: &'static str,
    /// Exact behavior the binding can promise for this preset.
    pub description: &'static str,
}
