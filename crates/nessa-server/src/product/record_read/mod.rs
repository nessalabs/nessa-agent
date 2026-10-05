//! Authorized record routing. The bounded wire representation is
//! `nessa_protocol::product::record_read`.
//! Dispatch and socket admission share the exhaustive read-refusal wire conversion, `refusal_code`.
mod dispatch;
pub(super) use dispatch::{dispatch, refusal_code};
