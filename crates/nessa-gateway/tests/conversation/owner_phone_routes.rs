//! Owner-phone product routes, over a paired phone from the conversation
//! crate's owner-phone fixture: deletes, channels, remembered receipts and the
//! receiver profile behind passive reads.
use crate::conversation::infrastructure::CedarOwnerPhonePolicyEvaluator;
use nessa_auth::adapters::cedar::CedarPolicyEvaluator;
use nessa_conversation::owner_phone_test_support::{fixture, ActualHolding, Time};
use std::sync::Arc;

#[path = "owner_phone_delete.rs"]
mod delete;

#[path = "owner_phone_channel.rs"]
mod channel;

#[path = "owner_phone_remember.rs"]
mod remember;

#[path = "owner_phone_profile.rs"]
mod owner_phone_profile;
