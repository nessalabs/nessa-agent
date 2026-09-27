use std::{error::Error, fmt};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UrlError {
    Whitespace,
    Invalid(::url::ParseError),
}
impl fmt::Display for UrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Whitespace => write!(f, "URL must not contain whitespace or control characters"),
            Self::Invalid(error) => write!(f, "invalid absolute URL: {error}"),
        }
    }
}
impl Error for UrlError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Invalid(error) => Some(error),
            Self::Whitespace => None,
        }
    }
}

/// Immutable absolute URL, parsed and normalized by the URL library.
/// Scheme restrictions belong to the feature that uses the URL. Parsing does no I/O.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Url(::url::Url);
impl Url {
    pub fn new(value: impl AsRef<str>) -> Result<Self, UrlError> {
        let value = value.as_ref();
        if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(UrlError::Whitespace);
        }
        ::url::Url::parse(value)
            .map(Self)
            .map_err(UrlError::Invalid)
    }
    /// Validate input without keeping a value object. Uses the same constructor rules.
    pub fn validate(value: &str) -> Result<(), UrlError> {
        Self::new(value).map(|_| ())
    }
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
    pub fn scheme(&self) -> &str {
        self.0.scheme()
    }
}
