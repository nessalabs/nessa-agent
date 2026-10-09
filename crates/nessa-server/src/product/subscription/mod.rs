//! Replay-to-live subscriptions on committed records
//! (`docs/design/record-subscriptions.md`). The connection owns each
//! subscription (`connection.rs`); each runs in its own task (`target.rs`),
//! which admits every batch in `authorize_batch` and reads through the same
//! fold a one-shot read uses; its frames reach the connection's one writer
//! through `delivery.rs`.

mod connection;
mod delivery;
mod target;

pub(super) use connection::ConnectionSubscriptions;
pub(super) use delivery::{SubscriptionDeliveries, SubscriptionFrame};
#[cfg(test)]
pub(crate) use target::LIST_REREAD_FLOOR;
