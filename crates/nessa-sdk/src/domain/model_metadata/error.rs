use super::value_objects::ModelKey;
use crate::domain::common::value_objects::{DateError, UrlError};
use std::{error::Error, fmt};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MetadataError {
    Invalid {
        field: &'static str,
        reason: &'static str,
    },
    InvalidDate(DateError),
    InvalidUrl(UrlError),
    UnsupportedProvider(String),
    EmptyCatalog,
    Duplicate(ModelKey),
}
impl fmt::Display for MetadataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { field, reason } => write!(f, "{field}: {reason}"),
            Self::InvalidDate(error) => error.fmt(f),
            Self::InvalidUrl(error) => error.fmt(f),
            Self::UnsupportedProvider(provider) => {
                write!(f, "unsupported model provider: {provider}")
            }
            Self::EmptyCatalog => write!(f, "a model catalog must contain at least one model"),
            Self::Duplicate(key) => {
                write!(
                    f,
                    "duplicate model: {}/{}",
                    key.provider().as_str(),
                    key.model_id()
                )
            }
        }
    }
}
impl Error for MetadataError {}
pub(super) fn invalid(field: &'static str, reason: &'static str) -> MetadataError {
    MetadataError::Invalid { field, reason }
}

impl From<DateError> for MetadataError {
    fn from(error: DateError) -> Self {
        Self::InvalidDate(error)
    }
}

impl From<UrlError> for MetadataError {
    fn from(error: UrlError) -> Self {
        Self::InvalidUrl(error)
    }
}
