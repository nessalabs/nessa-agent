use crate::{
    attachments::domain::{Attachment, Caller, UploadTicket},
    conversation::domain::ConversationId,
};
use nessa_auth::domain::OrganizationId;

/// Whether a conversation held a stored file before or after a transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldState {
    Absent,
    /// Written, but not yet usable: its evidence had not been committed.
    Pending,
    Held,
}

/// One conversation keeping one stored file. Its identity is the organization,
/// the conversation, and the stored file: digest and media type together. The
/// same bytes kept as two types are two holds, so declaring a file again as
/// something else never takes away the hold a sent message names.
///
/// A hold remembers two files that may differ. `uploaded` is what the caller
/// sent and the ticket verified; `stored` is what the gateway kept, which for
/// an image is the normalized result. Messages refer to `stored`; a repeated
/// upload is recognized by `uploaded`. Any other kind of file is stored as it
/// was uploaded, so there the two are equal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hold {
    organization_id: OrganizationId,
    conversation_id: ConversationId,
    uploaded: Attachment,
    stored: Attachment,
    uploaded_by: Caller,
    ticket_issued_at_ms: u64,
    uploaded_at_ms: u64,
}
impl Hold {
    /// The hold a verified upload earns. Owner, conversation, caller, and the
    /// uploaded file all come from the ticket, so a hold can only name what a
    /// ticket was issued for. `None` when `stored` could not have come from
    /// that upload: see [`Self::restore`].
    pub fn from_upload(
        ticket: &UploadTicket,
        stored: Attachment,
        uploaded_at_ms: u64,
    ) -> Option<Self> {
        Self::restore(
            ticket.organization_id().clone(),
            ticket.conversation_id().clone(),
            ticket.attachment().clone(),
            stored,
            ticket.caller().clone(),
            ticket.lifetime().issued_at_ms(),
            uploaded_at_ms,
        )
    }
    /// Rebuild a hold from saved evidence. Only an image is ever changed on
    /// its way into storage, and what it becomes is an image a message can
    /// name. A record saying a text file was stored as different bytes, or
    /// that an image became a file no message could refer to, describes an
    /// upload that cannot have happened and is refused.
    pub fn restore(
        organization_id: OrganizationId,
        conversation_id: ConversationId,
        uploaded: Attachment,
        stored: Attachment,
        uploaded_by: Caller,
        ticket_issued_at_ms: u64,
        uploaded_at_ms: u64,
    ) -> Option<Self> {
        let possible = if uploaded.media_type().is_image() {
            stored.as_image().is_some()
        } else {
            uploaded == stored
        };
        possible.then_some(Self {
            organization_id,
            conversation_id,
            uploaded,
            stored,
            uploaded_by,
            ticket_issued_at_ms,
            uploaded_at_ms,
        })
    }
    pub fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }
    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }
    pub fn uploaded(&self) -> &Attachment {
        &self.uploaded
    }
    pub fn stored(&self) -> &Attachment {
        &self.stored
    }
    pub fn uploaded_by(&self) -> &Caller {
        &self.uploaded_by
    }
    pub fn ticket_issued_at_ms(&self) -> u64 {
        self.ticket_issued_at_ms
    }
    pub fn uploaded_at_ms(&self) -> u64 {
        self.uploaded_at_ms
    }
    fn belongs_to(
        &self,
        organization_id: &OrganizationId,
        conversation_id: &ConversationId,
    ) -> bool {
        self.organization_id == *organization_id && self.conversation_id == *conversation_id
    }
    /// Whether this is that conversation keeping exactly that stored file.
    /// Owner, conversation, digest, media type, and size must all agree.
    pub fn keeps(
        &self,
        organization_id: &OrganizationId,
        conversation_id: &ConversationId,
        stored: &Attachment,
    ) -> bool {
        self.belongs_to(organization_id, conversation_id) && self.stored == *stored
    }
    /// Whether this hold came from that conversation uploading exactly that file.
    pub fn came_from(
        &self,
        organization_id: &OrganizationId,
        conversation_id: &ConversationId,
        uploaded: &Attachment,
    ) -> bool {
        self.belongs_to(organization_id, conversation_id) && self.uploaded == *uploaded
    }
}
