//! One private durable registration/outbox owner. Short SQLite transactions
//! retain state and causal audit together; publication never contains secrets.
mod audit;
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
mod store;
mod stored;

use codec::{integer, invalid, unavailable, uuid};
use schema::SCHEMA;
pub use store::SqlOutbox;
