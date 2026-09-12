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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelFeatures {
    input: Modalities,
    output: Modalities,
    tool_use: bool,
    reasoning: bool,
}
impl ModelFeatures {
    pub fn new(input: Modalities, output: Modalities, tool_use: bool, reasoning: bool) -> Self {
        Self {
            input,
            output,
            tool_use,
            reasoning,
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
}
