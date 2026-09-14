/// Provider-supplied review metadata. The adapter verifies the tool identity and
/// serializes its complete arguments. The host must interpret the tool's schema
/// before authorizing it; a display title or path alone is never enough.
/// JSON remains an opaque boundary representation, outside domain decision state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolReviewInput {
    /// Provider tool name validated by the selected adapter profile.
    pub name: String,
    /// Complete provider arguments retained for host review, not execution commands.
    pub arguments_json: String,
}
