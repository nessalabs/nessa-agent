//! Application boundary data. Domain types never depend on these DTOs.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModalitiesDto {
    pub text: bool,
    pub image: bool,
    pub audio: bool,
}

/// A reasoning model's published reasoning options.
///
/// Where a model does not reason, the field holding this is `null`: required,
/// so a catalogue entry that leaves it out is refused rather than read as a
/// model that does not reason.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReasoningDto {
    /// The effort levels the provider publishes, least effort first, in the
    /// provider's own names and order: never mapped onto another provider's.
    /// Each is a lowercase letter, then lowercase letters, digits, `-` or `_`,
    /// at most 32 bytes; at most 16 of them, none repeated. Empty when the
    /// model reasons but no level is recorded for it, so none is offered.
    pub effort_levels: Vec<String>,
}

/// Projection of the same immutable snapshot used for command validation.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveCapabilitiesDto {
    pub provider: String,
    pub model_id: String,
    pub input: ModalitiesDto,
    pub output: ModalitiesDto,
    pub tool_use: bool,
    /// Offered reasoning, `null` where the model or the binding offers none.
    /// Its levels are the model's published ones, present only where the
    /// binding can run them.
    pub reasoning: Option<ReasoningDto>,
    /// Fast mode, where both the model and the binding offer it.
    pub fast_mode: bool,
    pub context_window_tokens: u32,
    pub max_output_tokens: u32,
}

/// Published limits for one input image; see the domain `ImageInputLimits`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageInputLimitsDto {
    /// Accepted encodings as exact lowercase media types such as `image/png`:
    /// at least one, none repeated.
    pub media_types: Vec<String>,
    /// Largest single image as base64 text, the strictest across serving
    /// platforms. At least 4: one base64 group, the least that carries a byte.
    pub max_encoded_bytes: u64,
    /// Longest width or height, in pixels, the model accepts at all.
    pub max_edge_px: u32,
    /// The lower ceiling, in pixels, a provider applies to every image once a
    /// request holds many of them. At most `max_edge_px`.
    pub many_images_max_edge_px: u32,
    /// The long edge, in pixels, the model actually sees; a larger image is
    /// scaled down by the provider. At most `max_edge_px`.
    pub native_long_edge_px: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelMetadataDto {
    pub provider: String,
    pub model_id: String,
    pub display_name: String,
    pub input: ModalitiesDto,
    /// Present only with image input, and only once its limits are recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_input: Option<ImageInputLimitsDto>,
    pub output: ModalitiesDto,
    pub tool_use: bool,
    /// `null` for a model that does not reason; see [`ReasoningDto`]. The key
    /// is required even then.
    #[serde(deserialize_with = "Option::deserialize")]
    pub reasoning: Option<ReasoningDto>,
    /// Whether the provider publishes a fast mode for this model: faster
    /// output, not a reasoning effort level.
    pub fast_mode: bool,
    /// Published model ceiling, not the binding's default/configured context window
    /// or an independent maximum input allowance.
    pub max_context_window_tokens: u32,
    /// Standard API output ceiling, excluding beta/batch extensions.
    pub max_output_tokens: u32,
    /// Provider's published precision: YYYY-MM or YYYY-MM-DD.
    pub knowledge_cutoff: String,
    pub documentation_url: String,
}
