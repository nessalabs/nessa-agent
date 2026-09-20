use super::UploadRejection;

/// Why an upload could not begin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeginError {
    /// A field did not describe a conversation, a file, or an action.
    InvalidRequest,
    /// No such conversation for this caller. Another owner's conversation and
    /// one that does not exist are deliberately the same answer.
    ConversationNotFound,
    /// The upload says it is an image, and the selected model is offered none,
    /// so no image is prepared for it. Nothing is wrong with the file; there is
    /// simply no message that could ever name it.
    ImagesUnsupported,
    /// As many tickets are outstanding as this gateway allows: in all, for
    /// this organization, or for this conversation.
    Capacity,
    /// Existing holds could not be read.
    Storage,
    /// A ticket that expired or was replaced is gone, or a new ticket was made,
    /// and the audit sink did not acknowledge it. No ticket was handed out.
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
    /// used, replaced, withdrawn, or already swept. Nothing changed and there
    /// is nobody to attribute it to.
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
    /// The upload was good and could not be recorded in the audit trail, so it
    /// is not kept: a hold this upload wrote is taken back, and a file the
    /// conversation already kept is left exactly as it was. Which of the two
    /// happened, and how far taking back got, is in the trail and the log; the
    /// caller's remedy is the same either way, which is to begin again.
    AuditUnavailable,
    /// The upload's hold was removed before it became usable: its conversation
    /// let go of its files meanwhile, or a concurrent upload of the same file
    /// that had taken the hold over took it back. The creation on record is
    /// followed by a record that it did not last. Beginning again is safe.
    NotKept,
}

/// Why a release did not complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseError {
    /// The caller cannot be written down, so nothing was done: letting go of
    /// files in a name no record can carry is not something this context does.
    Unattributable,
    /// Every part was tried. These did not complete. The two counts answer
    /// different questions, and the close route answers each with its own code:
    /// files still in place are a cleanup the caller retries, while files that
    /// went without evidence are a lost record.
    Incomplete {
        /// Holds or bytes that could not be read or removed. A store that
        /// could not be asked at all counts as one.
        storage_failures: usize,
        /// Transitions that happened but whose evidence was not acknowledged,
        /// including records the release's audit budget did not reach.
        audit_failures: usize,
    },
}
