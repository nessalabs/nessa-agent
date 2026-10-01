use std::{error::Error, fmt};

/// A value an MCP server sent about an app, refused when it was made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpAppError {
    /// The URI does not start with `ui://`, the scheme MCP Apps resources use.
    NotUiUri,
    /// A value holds more UTF-8 bytes than it may.
    ValueTooLong {
        /// Which value.
        field: &'static str,
        /// The most bytes it may hold.
        max_bytes: usize,
    },
    /// A list holds more entries than it may.
    TooManyValues {
        /// Which list.
        field: &'static str,
        /// The most entries it may hold.
        max: usize,
    },
    /// A value is empty, or holds a character it may not: whitespace or a
    /// control character in a URI, anything outside a CSP source's alphabet.
    InvalidValue(&'static str),
}
impl fmt::Display for McpAppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotUiUri => f.write_str("not a ui:// URI"),
            Self::ValueTooLong { field, max_bytes } => {
                write!(f, "{field} is longer than {max_bytes} bytes")
            }
            Self::TooManyValues { field, max } => write!(f, "{field} has more than {max} entries"),
            Self::InvalidValue(field) => {
                write!(f, "{field} is empty or holds a character it may not")
            }
        }
    }
}
impl Error for McpAppError {}
