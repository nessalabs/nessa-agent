//! GitHub CLI credential, HTTPS/WebSocket and private SQLite adapters.
mod credentials;
mod sqlite;
mod transport;
pub use credentials::ConnectedGithubCredentials;
pub use sqlite::SqliteRelayStore;
pub use transport::{relay_configuration, HttpRelaySource, RelayConfiguration};

mod lifecycle;
pub use lifecycle::{TokioRelayWait, TracingRelayReporter};
mod connected_store;
pub use connected_store::ConnectedRelayStore;
mod selection;

mod keychain;
pub use keychain::KeychainInboxKey;
