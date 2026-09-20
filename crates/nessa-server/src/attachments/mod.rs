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
//! keeps one stored file, and remembers both files: the one uploaded (so a
//! repeated upload is recognized) and the one stored (so a message can be
//! checked). The conversation context asks `holds` before accepting a message,
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
//! two times: hold created, upload refused, ticket expired unused (automatic,
//! and labelled so), ticket withdrawn, hold released, bytes removed. A hold
//! whose creation could not be recorded is taken back. A release whose
//! evidence could not be recorded still releases, and says so.
//!
//! What this does not do. It cannot know which holds a sent message used, so a
//! file that was uploaded and never sent is not expired on its own: holds live
//! until their conversation closes. What does expire is an unused ticket, and
//! temporary files are swept once, when the store is opened. Tickets are not
//! durable: a restart forgets them, without a record. An upload that was
//! already past its ticket when its conversation closed still completes, and
//! its hold then lives until that conversation is closed again. Bytes published
//! just before a crash, or whose removal failed, stay until a later upload of
//! the same bytes is held and released. Nothing bounds how much one owner keeps.
pub mod application;
pub mod domain;
pub mod entrypoint;
pub mod infrastructure;
