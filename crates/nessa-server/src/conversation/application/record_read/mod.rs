//! Fresh passive record-read application contracts and orchestration.
mod read;
pub(crate) use read::validate_record_selector;
pub use read::{
    ReadRecords, RecordHead, RecordReadError, RecordReadFuture, RecordReadLease,
    RecordReadOperation, RecordReadResponse, RecordReadSource, RecordReadValue,
};
