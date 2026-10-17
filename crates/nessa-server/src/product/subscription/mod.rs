//! Replay-to-live subscriptions on committed records
//! (`docs/design/record-subscriptions.md`). The connection owns each
//! subscription (`connection.rs`); each runs in its own task (`target.rs`),
//! which admits every batch in `authorize_batch` and reads through the same
//! fold a one-shot read uses; its frames reach the connection's one writer
//! through `delivery.rs`. `target::Sources` holds the separately published
//! per-principal subscription allowance until its physical sources drop.
//! The agent catalog (`agents.subscribe`) is one more
//! target of the same machinery, woken by the paired environments'
//! connection changes rather than by records (`docs/design/auth/peer-gateways.md`,
//! row I10).

mod connection;
mod delivery;
mod events;
mod target;

pub(super) use connection::ConnectionSubscriptions;
pub(super) use delivery::{SubscriptionDeliveries, SubscriptionFrame};
#[cfg(test)]
pub(in crate::product) use events::read_event;
#[cfg(test)]
pub(in crate::product) use target::Target;
#[cfg(test)]
pub(crate) use target::LIST_REREAD_FLOOR;
