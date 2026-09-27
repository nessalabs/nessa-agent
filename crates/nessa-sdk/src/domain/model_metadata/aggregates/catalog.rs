use super::super::entities::ModelMetadata;
use super::super::value_objects::ModelKey;
use super::super::{error::invalid, MetadataError};
use crate::domain::common::value_objects::Date;
use std::collections::HashSet;

/// Catalog aggregate: construction protects unique identities across its model entities.
/// It owns no persistence, mutable lifecycle, or domain event machinery.
#[derive(Clone, Debug)]
pub struct Catalog {
    verified_on: Date,
    models: Vec<ModelMetadata>,
}
impl Catalog {
    pub fn new(verified_on: Date, models: Vec<ModelMetadata>) -> Result<Self, MetadataError> {
        if !verified_on.is_day() {
            return Err(invalid("verification date", "requires day precision"));
        }
        if models.is_empty() {
            return Err(MetadataError::EmptyCatalog);
        }
        let mut keys = HashSet::new();
        for model in &models {
            if !keys.insert(model.key()) {
                return Err(MetadataError::Duplicate(model.key().clone()));
            }
        }
        Ok(Self {
            verified_on,
            models,
        })
    }
    pub fn verified_on(&self) -> &Date {
        &self.verified_on
    }
    pub fn models(&self) -> &[ModelMetadata] {
        &self.models
    }
    pub fn find(&self, key: &ModelKey) -> Option<&ModelMetadata> {
        self.models.iter().find(|model| model.key() == key)
    }
}
