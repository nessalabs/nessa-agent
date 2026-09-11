//! Application boundary data. Domain types never depend on these DTOs.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModalitiesDto {
    pub text: bool,
    pub image: bool,
    pub audio: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelMetadataDto {
    pub provider: String,
    pub model_id: String,
    pub display_name: String,
    pub input: ModalitiesDto,
    pub output: ModalitiesDto,
    pub tool_use: bool,
    pub reasoning: bool,
    /// Published model ceiling, not the binding's default/configured context window
    /// or an independent maximum input allowance.
    pub max_context_window_tokens: u32,
    /// Standard API output ceiling, excluding beta/batch extensions.
    pub max_output_tokens: u32,
    /// Provider's published precision: YYYY-MM or YYYY-MM-DD.
    pub knowledge_cutoff: String,
    pub documentation_url: String,
}
