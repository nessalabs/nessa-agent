use super::super::{error::invalid, MetadataError};
use crate::domain::common::value_objects::{Date, Url};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelDescription {
    display_name: String,
    knowledge_cutoff: Date,
    documentation_url: Url,
}
impl ModelDescription {
    pub fn new(
        display_name: String,
        knowledge_cutoff: Date,
        documentation_url: Url,
    ) -> Result<Self, MetadataError> {
        if display_name.trim().is_empty() {
            return Err(invalid("display name", "must not be blank"));
        }
        if documentation_url.scheme() != "https" {
            return Err(invalid("documentation URL", "requires HTTPS"));
        }
        Ok(Self {
            display_name,
            knowledge_cutoff,
            documentation_url,
        })
    }
    pub fn display_name(&self) -> &str {
        &self.display_name
    }
    pub fn knowledge_cutoff(&self) -> &Date {
        &self.knowledge_cutoff
    }
    pub fn documentation_url(&self) -> &Url {
        &self.documentation_url
    }
}
