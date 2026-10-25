//! One private durable registration/outbox owner. Short SQLite transactions
//! retain state and causal audit together; publication never contains secrets.
//! `intents -> cancellation -> retirement` removes an uninstalled reservation;
//! `registry -> journal -> retirement` drains an installed registration. The
//! retained terminal kind and shared history validation keep those facts apart.
mod audit;
mod cancellation;
mod capacity;
mod codec;
mod cursor;
mod intents;
mod journal;
mod publication;
mod queue;
mod registry;
mod restoration;
mod retirement;
mod schema;
mod source_reset;
mod stop;
mod store;
mod stored;

use codec::{integer, invalid, unavailable, uuid};
use schema::SCHEMA;
pub use store::SqlOutbox;
