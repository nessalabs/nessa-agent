//! Coordinates provider reads and local atomic page commits through injected ports.
mod catch_up;
mod follow;
mod ports;
pub use catch_up::catch_up;
pub use follow::{follow, serve};
pub use ports::{
    GitHubCredentials, RelayLiveSource, RelayNotifications, RelayReporter, RelaySource, RelayStore,
    RelayWait,
};
mod live_events;
pub use live_events::{ConnectedGithub, LiveEventStore, LiveEvents};
