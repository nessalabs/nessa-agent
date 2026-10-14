//! One JSON line is one record. A line that is not JSON is skipped.

use crate::backup::domain::{
    CapturedRecord, RecordError, Sensitivity, SkipReason, SkippedRecord, SourceCut, StoreKind,
    Watermark,
};

/// `text` is the file. Line numbers start at 1. A line that does not parse
/// as a JSON value is skipped and named `line-N`.
pub fn jsonl_cut(store: StoreKind, text: &str) -> Result<SourceCut, RecordError> {
    let mut records = Vec::new();
    let mut skipped = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let identity = format!("line-{}", index + 1);
        if serde_json::from_str::<serde_json::Value>(line).is_ok() {
            records.push(CapturedRecord::new(
                store,
                identity,
                Sensitivity::Public,
                line.as_bytes().to_vec(),
            )?);
        } else {
            skipped.push(SkippedRecord::new(store, identity, SkipReason::Unreadable)?);
        }
    }
    let watermark = Watermark::new(text.lines().count().to_string())?;
    if text.is_empty() {
        return SourceCut::new(store, Watermark::new("0")?, records, skipped);
    }
    SourceCut::new(store, watermark, records, skipped)
}
