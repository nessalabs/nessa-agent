//! Which modalities and features a model is published with, or a binding can
//! deliver.
#![deny(missing_docs)]

use super::super::{error::invalid, MetadataError};

/// The kinds of content one side of a model (its input or its output) takes:
/// at least one of text, image and audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Modalities {
    text: bool,
    image: bool,
    audio: bool,
}
impl Modalities {
    /// Whether text, images and audio are taken.
    ///
    /// # Errors
    ///
    /// [`MetadataError::Invalid`] naming `modalities` when all three are false.
    pub fn new(text: bool, image: bool, audio: bool) -> Result<Self, MetadataError> {
        if !text && !image && !audio {
            return Err(invalid("modalities", "must support at least one modality"));
        }
        Ok(Self { text, image, audio })
    }
    /// Whether text is taken.
    pub fn text(self) -> bool {
        self.text
    }
    /// Whether images are taken.
    pub fn image(self) -> bool {
        self.image
    }
    /// Whether audio is taken.
    pub fn audio(self) -> bool {
        self.audio
    }
}

/// Which features a model is published with, or a binding can deliver.
///
/// The same flags describe both, so a binding's declaration is a ceiling the
/// model's facts are intersected with. What a feature offers in detail — a
/// reasoning model's effort levels, an image model's limits — is recorded on
/// the model entity, and only where the flag here is set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelFeatures {
    input: Modalities,
    output: Modalities,
    tool_use: bool,
    reasoning: bool,
    fast_mode: bool,
}
impl ModelFeatures {
    /// - `input`, `output`: the content the model takes and produces, or the
    ///   binding can pass each way.
    /// - `tool_use`: the model calls tools, or the binding lets it.
    /// - `reasoning`: the model reasons before answering, or the binding can
    ///   run a reasoning model's reasoning selection.
    /// - `fast_mode`: the provider offers a faster output mode for the model,
    ///   or the binding can turn it on. Speed, not reasoning effort.
    pub fn new(
        input: Modalities,
        output: Modalities,
        tool_use: bool,
        reasoning: bool,
        fast_mode: bool,
    ) -> Self {
        Self {
            input,
            output,
            tool_use,
            reasoning,
            fast_mode,
        }
    }
    /// What the model takes, or the binding passes in.
    pub fn input(self) -> Modalities {
        self.input
    }
    /// What the model produces, or the binding passes back.
    pub fn output(self) -> Modalities {
        self.output
    }
    /// Whether the model calls tools, or the binding lets it.
    pub fn tool_use(self) -> bool {
        self.tool_use
    }
    /// Whether the model reasons, or the binding can run its reasoning
    /// selection. A reasoning model's levels are on its entity
    /// ([`crate::domain::model_metadata::entities::ModelMetadata::effort_levels`]).
    pub fn reasoning(self) -> bool {
        self.reasoning
    }
    /// Whether the provider publishes a fast mode for the model (faster
    /// output at a higher price, not more or less reasoning), or the binding
    /// can turn it on. Every binding declares it off today, since none sends
    /// it to its agent.
    pub fn fast_mode(self) -> bool {
        self.fast_mode
    }
}
