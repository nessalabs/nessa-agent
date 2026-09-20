use super::UploadRejection;

/// Why an upload could not begin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeginError {
    /// A field did not describe a conversation, a file, or an action.
    InvalidRequest,
    /// No such conversation for this caller. Another owner's conversation and
    /// one that does not exist are deliberately the same answer.
    ConversationNotFound,
    /// As many tickets are outstanding as this gateway allows.
    Capacity,
    /// Existing holds could not be read.
    Storage,
    /// Tickets that expired were removed, but their expiry could not be recorded.
    Audit,
    /// Ownership could not be looked up, or no secret could be made.
    Unavailable,
}

/// Whether the evidence of a refusal reached the audit sink.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditDelivery {
    Recorded,
    Unavailable,
}

/// Why an upload did not become a hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadError {
    /// No outstanding ticket has this secret: malformed, never issued, already
    /// used, or already swept. Nothing changed and there is nobody to attribute it to.
    TicketInvalid,
    /// The ticket existed but its time had passed. It has been removed.
    TicketExpired { evidence: AuditDelivery },
    /// Too many uploads are in progress. The ticket was not touched.
    Busy,
    /// The ticket was used up and the upload refused. The reason stays the
    /// primary failure even when recording it failed too.
    Rejected {
        reason: UploadRejection,
        evidence: AuditDelivery,
    },
    /// The upload was good, but the hold could not be recorded in the audit
    /// trail, so it was taken back. `reverted` is false when taking it back
    /// failed as well and the hold may remain.
    AuditUnavailable { reverted: bool },
}

/// What a release could not complete, after every part of it was tried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReleaseError {
    /// Holds or bytes that could not be read or removed. A store that could
    /// not be asked at all counts as one.
    pub storage_failures: usize,
    /// Transitions that happened but whose evidence was not acknowledged.
    pub audit_failures: usize,
}
