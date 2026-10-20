//! One quantum, and only after [`crate::read_only_sync::domain::LinkSchedule`] admits it.
//!
//! The quote is the published ceiling for that interest, known before the
//! read. Record and history pages use the page limit the driver will use.
//! The catalogue uses its payload ceiling. An artifact range uses
//! `HELD_FILE_RANGE_BYTES`.
use crate::read_only_sync::domain::{
    Admit, Candidate, Interest, LinkSchedule, Quote, Settle, HELD_FILE_RANGE_BYTES,
};
use nessa_sync::replication::catalogue::MAX_CATALOGUE_PAYLOAD_BYTES;

/// The conservative payload quote for `interest`. Always known: the caller
/// has already classified it.
pub(crate) fn conservative_quote(interest: &Interest, page_payload_bytes: u64) -> Quote {
    let bytes = match interest {
        Interest::OpenTranscript(_) | Interest::OlderHistory(_) => page_payload_bytes,
        Interest::Catalogue => u64::try_from(MAX_CATALOGUE_PAYLOAD_BYTES).unwrap_or(u64::MAX),
        Interest::Artifact { .. } => HELD_FILE_RANGE_BYTES,
    };
    Quote::Known(bytes)
}

/// Admit at most one quantum. The closure runs only for [`Admit::Go`], and
/// its return value is the payload bytes the cache kept.
pub(crate) fn run_quantum(
    schedule: LinkSchedule,
    candidates: &[Candidate],
    quantum: &mut dyn FnMut(&Interest) -> u64,
) -> (LinkSchedule, Admit, Option<Settle>) {
    let (schedule, admit) = schedule.admit(candidates);
    match &admit {
        Admit::Go { interest, .. } => {
            let kept = quantum(interest);
            let (schedule, settled) = schedule.finish(kept);
            (schedule, admit, Some(settled))
        }
        _ => (schedule, admit, None),
    }
}

#[cfg(test)]
#[path = "../../../tests/read_only_sync/application/link.rs"]
mod tests;
