//! Immutable, validated values of the gateway context.
mod search_path;
pub use search_path::{SearchPath, SearchPathError};
mod service_configuration;
pub use service_configuration::{
    ReconciliationCause, ServiceConfiguration, ServiceConfigurationError,
};
