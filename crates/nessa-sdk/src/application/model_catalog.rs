//! Catalog construction and queries. Domain objects own every metadata invariant;
//! this layer maps boundary DTOs and adds actionable application errors.
use super::dto::ModelMetadataDto;
use crate::domain::common::value_objects::Date;
use crate::domain::model_metadata::{
    aggregates::Catalog,
    entities::ModelMetadata,
    value_objects::{ModelKey, ModelProvider},
    MetadataError,
};
use std::{error::Error, fmt};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogError {
    Invalid {
        entry: Option<usize>,
        source: MetadataError,
    },
    Duplicate {
        provider: String,
        model_id: String,
    },
    ModelNotFound {
        provider: String,
        model_id: String,
    },
}
impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { entry: Some(index), source } => write!(f, "model metadata entry {index}: {source}"),
            Self::Invalid { entry: None, source } => write!(f, "model metadata: {source}"),
            Self::Duplicate { provider, model_id } => write!(f, "duplicate model metadata: {provider}/{model_id}"),
            Self::ModelNotFound { provider, model_id } => write!(f, "model metadata missing for {provider}/{model_id}; add an entry to the catalog and restart"),
        }
    }
}
impl Error for CatalogError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Invalid { source, .. } => Some(source),
            _ => None,
        }
    }
}
fn catalog_error(source: MetadataError) -> CatalogError {
    match source {
        MetadataError::Duplicate(key) => CatalogError::Duplicate {
            provider: key.provider().as_str().into(),
            model_id: key.model_id().into(),
        },
        source => CatalogError::Invalid {
            entry: None,
            source,
        },
    }
}

/// Scoped query application, constructed with a validated domain catalog.
/// DTOs returned to callers are projections, never authoritative mutable state.
#[derive(Clone, Debug)]
pub struct ModelCatalog {
    catalog: Catalog,
}
impl ModelCatalog {
    pub fn new(catalog: Catalog) -> Self {
        Self { catalog }
    }

    /// Import application input through the domain's validated constructors.
    pub fn from_metadata(
        verified_on: String,
        models: Vec<ModelMetadataDto>,
    ) -> Result<Self, CatalogError> {
        let verified_on = Date::new(verified_on)
            .map_err(MetadataError::from)
            .map_err(catalog_error)?;
        let models = models
            .into_iter()
            .enumerate()
            .map(|(index, dto)| {
                ModelMetadata::try_from(dto).map_err(|source| CatalogError::Invalid {
                    entry: Some(index),
                    source,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::new(
            Catalog::new(verified_on, models).map_err(catalog_error)?,
        ))
    }
    pub fn verified_on(&self) -> &str {
        self.catalog.verified_on().as_str()
    }
    /// Display order follows the imported catalog.
    pub fn models(&self) -> Vec<ModelMetadataDto> {
        self.catalog
            .models()
            .iter()
            .map(ModelMetadataDto::from)
            .collect()
    }
    pub fn select(&self, provider: &str, model_id: &str) -> Result<ModelMetadataDto, CatalogError> {
        let key = ModelKey::new(
            ModelProvider::try_from(provider).map_err(catalog_error)?,
            model_id.into(),
        )
        .map_err(catalog_error)?;
        self.catalog
            .find(&key)
            .map(ModelMetadataDto::from)
            .ok_or_else(|| CatalogError::ModelNotFound {
                provider: provider.into(),
                model_id: model_id.into(),
            })
    }
}
