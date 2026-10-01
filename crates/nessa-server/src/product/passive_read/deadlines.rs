//! Server passive source and queued-delivery phase budgets.
//! The schema also publishes their sum with the client allowance as a request floor.
use crate::product::generated::{PASSIVE_DELIVERY_TIMEOUT_MS, PASSIVE_READ_TIMEOUT_MS};
use std::time::Duration;

pub(crate) const PASSIVE_READ_TIMEOUT: Duration = Duration::from_millis(PASSIVE_READ_TIMEOUT_MS);
pub(crate) const RECORD_SEND_TIMEOUT: Duration = Duration::from_millis(PASSIVE_DELIVERY_TIMEOUT_MS);
