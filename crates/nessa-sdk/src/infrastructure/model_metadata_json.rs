//! JSON boundary. The host opens its chosen file and passes a reader at startup.

use crate::application::{
    dto::ModelMetadataDto,
    model_catalog::{CatalogError, ModelCatalog},
};
use serde::Deserialize;
use std::{error::Error, fmt, io::Read};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogDocument {
    verified_on: String,
    models: Vec<ModelMetadataDto>,
}

#[derive(Debug)]
pub enum LoadCatalogError {
    Json(serde_json::Error),
    Invalid(CatalogError),
}

impl fmt::Display for LoadCatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(f, "cannot read model metadata JSON: {error}"),
            Self::Invalid(error) => error.fmt(f),
        }
    }
}

impl Error for LoadCatalogError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(match self {
            Self::Json(error) => error,
            Self::Invalid(error) => error,
        })
    }
}

pub fn load_catalog(reader: impl Read) -> Result<ModelCatalog, LoadCatalogError> {
    let document: CatalogDocument =
        serde_json::from_reader(reader).map_err(LoadCatalogError::Json)?;
    ModelCatalog::from_metadata(document.verified_on, document.models)
        .map_err(LoadCatalogError::Invalid)
}
