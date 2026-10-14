//! Consumer policy and cache refusals, independent of SQLite and sockets.
//! Resource policy arrives from composition; core and SDK types own semantics.
//! `driver` schedules finite pages through core ports; `offline` returns saved
//! positions and the shared bounded conversation view through one read port.
//! `reset` exposes attributed durable reset receipts through the cache port.
//! `device` turns the gateway's pinned enrollment status into a read, a purge
//! with its receipt, or neither. `watch` runs the bounded loop of one record
//! watch: registration, the recheck pass, then one pass per hint.
//! `held_file` fetches one file a conversation holds, range by range, and
//! keeps it only once its size and digest are the ones named.

mod cache;
mod catalogue;
pub(crate) mod device;
pub(crate) mod driver;
pub(crate) mod held_file;
pub(crate) mod offline;
pub(crate) mod reset;
pub(crate) mod watch;
pub(crate) use cache::{
    CacheError, CachePolicy, CachedProgress, CatalogueResetReceipt, ResetReceipt,
};
pub(crate) use catalogue::{CachedCatalogueEntry, CachedCataloguePage};

mod gateway;
pub(crate) use gateway::{
    Cancellation, GatewayAttempt, GatewayConnector, GatewayError, GatewayOutcome, GatewayPolicy,
    GatewayStream, HeldFileRefusal, SubscriptionCode, SubscriptionRefusal,
};
