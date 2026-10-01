//! Server passive source and queued-delivery phase budgets.
//! The default example composes these phases into its whole-operation allowance.
use std::time::Duration;

pub(crate) const PASSIVE_READ_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const RECORD_SEND_TIMEOUT: Duration = Duration::from_secs(30);
