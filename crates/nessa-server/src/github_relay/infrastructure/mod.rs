//! GitHub CLI credential, HTTPS/WebSocket and private SQLite adapters.
mod credentials;
mod sqlite;
mod transport;
pub use credentials::GhCredentials;
pub use sqlite::SqliteRelayStore;
pub use transport::{relay_configuration, HttpRelaySource, RelayConfiguration};

mod lifecycle;
pub use lifecycle::{TokioRelayWait, TracingRelayReporter};
