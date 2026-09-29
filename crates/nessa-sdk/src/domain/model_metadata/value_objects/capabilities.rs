use super::super::{error::invalid, MetadataError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Modalities {
    text: bool,
    image: bool,
    audio: bool,
}
impl Modalities {
    pub fn new(text: bool, image: bool, audio: bool) -> Result<Self, MetadataError> {
        if !text && !image && !audio {
            return Err(invalid("modalities", "must support at least one modality"));
        }
        Ok(Self { text, image, audio })
    }
    pub fn text(self) -> bool {
        self.text
    }
    pub fn image(self) -> bool {
        self.image
    }
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
    pub fn input(self) -> Modalities {
        self.input
    }
    pub fn output(self) -> Modalities {
        self.output
    }
    pub fn tool_use(self) -> bool {
        self.tool_use
    }
    pub fn reasoning(self) -> bool {
        self.reasoning
    }
    pub fn fast_mode(self) -> bool {
        self.fast_mode
    }
}
