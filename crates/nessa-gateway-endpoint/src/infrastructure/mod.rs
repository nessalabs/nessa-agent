//! Private JSON-file adapters for endpoint publication and health correlation.

mod file;

pub use file::{FileEndpointDiscovery, FileEndpointPublication, ENDPOINT_FILE};
