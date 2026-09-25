//! Files a conversation uploads so its messages can refer to them.
//!
//! A message never carries bytes. The product socket is capped at 64 KiB a
//! frame, and a request is compared and persisted whole, so a message carries a
//! reference (digest, media type, size) and the bytes travel separately:
//!
//! 1. Over the authenticated socket, `attachment.begin` describes the file. If
//!    this conversation already uploaded exactly that file, the answer is the
//!    stored reference and nothing is sent. Otherwise it is a ticket: 32 random
//!    bytes, usable once, for five minutes, for that file, that conversation,
//!    and that caller. Only the ticket's SHA-256 is kept, and only in memory.
//!    Issuing it is recorded before it is handed out. The same request made
//!    again replaces its ticket rather than adding one, and tickets are bounded
//!    in all, per organization, and per conversation.
//! 2. `PUT /attachments` sends the bytes with the ticket in a header. The route
//!    authenticates nobody: the ticket is the whole authority, and it is used up
//!    the moment it is found, whatever happens next. The body streams to a
//!    private temporary file, hashed as it arrives, and is abandoned as soon as
//!    it runs longer than the ticket said. It must be exactly the described
//!    size and digest.
//! 3. An image is then normalized through the `ImageNormalizer` port (an
//!    encoding and dimensions the selected model accepts), so what is kept can
//!    differ from what was sent. Any other file is kept as it arrived. The
//!    answer is the stored reference, which is what `conversation.send` names.
//!
//! Bytes are stored once per digest. A *hold* records that one conversation
//! keeps one stored file (digest and media type together, so the same bytes
//! kept as two types are two holds and neither replaces the other), and
//! remembers both files: the one uploaded (so a repeated upload is recognized)
//! and the one stored (so a message can be checked). A usable hold is never
//! overwritten.
//!
//! A hold's creation has one owner, the upload that made it, and one order:
//! the hold is written *pending*, which nothing that asks can see; its creation
//! is recorded; only then is it made usable. The store gives the upload a claim
//! on exactly the record it wrote, and only that claim confirms it or takes it
//! back. So an upload whose evidence failed undoes its own write and nothing
//! else: not a later upload of the same file that was recorded and answered,
//! and not a record a release already removed. A release removes pending holds
//! too, and the upload waiting on one is told its file was not kept. A creation
//! that may have reached the trail and did not last is followed by
//! `attachment_hold_reverted`, so the trail never ends on "held" for a hold
//! that is gone.
//! The conversation context asks `holds` before accepting a message,
//! which is why a digest learned elsewhere reads nothing; the agent adapter
//! then reads bytes by content alone. Closing a conversation withdraws its
//! unused tickets and releases its holds, and bytes go when their last hold does.
//!
//! ```text
//!  product socket --attachment.begin--> application --> domain (TicketBook)
//!  PUT /attachments (entrypoint) -----> application --> domain (UploadTicket, Hold)
//!                                            |
//!              ports: AttachmentStore, AttachmentAudit, ImageNormalizer,
//!                     ConversationOwnership, TicketSecrets, Clock
//!                                            ^
//!                                     infrastructure
//!  conversation::ConversationAttachments <---|---> sdk::UserImageSource
//! ```
//!
//! Downward arrows are calls. The upward arrow is "implements": infrastructure
//! fills the application's ports, and also implements the conversation
//! context's and the SDK's own ports on top of this context.
//!
//! Every consequential transition is handed to `AttachmentAudit` with its
//! target, both digests, before and after, cause, initiator, correlation, and
//! two times: ticket issued, replaced, expired unused (automatic, and labelled
//! so) or withdrawn; upload refused; hold created, already held, reverted
//! (automatic) or released; bytes removed. Who is acting is settled before the
//! first effect of every entry point, under the same rule the conversation
//! context applies to a caller, so nothing is ever let go in a name no record
//! can carry. A release whose evidence could not be recorded still releases,
//! and says so. A refusal has one name on the wire, in the trail, and in the
//! guide.
//!
//! What this does not do. It cannot know which holds a sent message used, so a
//! file that was uploaded and never sent is not expired on its own: holds live
//! until their conversation closes. What does expire is an unused ticket, and
//! temporary files are swept once, when the store is opened. Tickets are not
//! durable: a restart forgets them, without a record. Closing is not final: a
//! conversation can be reopened and nothing in its ownership record says it was
//! closed, so it can hold again after a close, and what it holds then is let go
//! by the next close. That includes an upload still transferring when its
//! conversation closed. Deleting is final: a deleted conversation begins no
//! upload, and ownership is asked again once an upload's hold is written, so
//! one still transferring when its conversation was deleted keeps nothing. A ticket outlives the revocation of the credential it
//! was given to, for at most its five minutes: the route authenticates nobody,
//! and re-deciding access there would be a second, weaker copy of the socket's
//! authorization. A pending hold left by a crash stays invisible and protects
//! its bytes until the same file is uploaded again or its conversation closes.
//! Bytes whose removal failed stay until a later upload of the same bytes is
//! held and released. Transfers are bounded across all callers, not per
//! organization: this gateway serves exactly one. Nothing bounds how much one
//! owner keeps.
pub mod application;
pub mod domain;
pub mod entrypoint;
pub mod infrastructure;

#[cfg(test)]
#[path = "../../tests/attachments/agreement.rs"]
mod agreement_tests;
