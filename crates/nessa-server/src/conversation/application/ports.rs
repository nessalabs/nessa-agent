use super::ConversationError;
use crate::conversation::domain::{
    Conversation, ConversationDeletion, ConversationId, ConversationSummary, ProviderSessionErasure,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::domain::agent_execution::{prompts::ImageReference, sessions::ExecutionSessionId};
use std::{future::Future, pin::Pin};

pub type ConversationFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ConversationError>> + Send + 'a>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationCreationDisposition {
    Created,
    Existing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationCreation {
    pub conversation: Conversation,
    pub disposition: ConversationCreationDisposition,
}

/// Immutable evidence for one acknowledged create or reopen request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationCreationAuditRecord {
    pub conversation_id: ConversationId,
    pub organization_id: OrganizationId,
    pub owner_id: PrincipalId,
    pub before: ConversationOwnershipState,
    pub after: ConversationOwnershipState,
    pub cause: ConversationCreationCause,
    pub initiator_principal_id: PrincipalId,
    pub initiator_surface_id: String,
    pub correlation_id: String,
    pub requested_at_ms: u64,
    pub observed_at_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationOwnershipState {
    Absent,
    Owned,
    /// Deleted for good. The identity keeps a tombstone and is not reused
    /// (`a_deleted_conversation_refuses_every_command_on_it`).
    Deleted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationCreationCause {
    CallerRequested,
    IdempotentReopen,
}

/// Commits creation evidence before the application reports success.
/// Caller-requested creation records are idempotent by conversation identity so
/// an interrupted first audit delivery can be safely retried from stored evidence.
pub trait ConversationCreationAudit: Send + Sync {
    fn record(&self, record: ConversationCreationAuditRecord) -> ConversationFuture<'_, ()>;
}

/// Immutable evidence that a conversation was deleted, committed before any of
/// its history is erased.
///
/// Built from the stored tombstone, not from whichever request is asking: a
/// delete repeated to finish an incomplete erasure — or the gateway finishing
/// it after a restart — records the one deletion in the name of the request
/// that decided it, so the record is the same each time
/// (`a_deleted_conversation_refuses_every_command_on_it`,
/// `an_unfinished_deletion_is_finished_and_recorded_when_the_gateway_starts`).
/// `correlation_id` names that request, which is allowed for the reason
/// [`ConversationFileLinkAuditRecord`] gives: the record is *of* it.
///
/// `provider_session_id` is what the conversation's saved history named as the
/// provider's own session, read after the agent was confirmed stopped and
/// before the history is erased. It is the only link that survives from this
/// conversation to execution evidence keyed by that session, and to whatever
/// the agent kept of it. `None` means the history named none: the agent never
/// attached.
///
/// `provider_erasure` is what became of the agent's own record of that
/// session, as the exchange with the agent confirmed it — never more: an
/// agent that acknowledged a delete may have archived rather than erased
/// (`the_agents_own_record_is_asked_to_go_after_the_stop_and_before_the_history`).
/// The record is written only once that is settled, so it says one thing and
/// is never contradicted by a retry.
///
/// There is no observation time here: the sink assigns it when it commits the
/// record, from its own clock. `requested_at_ms` is the service's reading when
/// the deciding request arrived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationDeletionAuditRecord {
    pub conversation_id: ConversationId,
    pub organization_id: OrganizationId,
    pub owner_id: PrincipalId,
    pub before: ConversationOwnershipState,
    pub after: ConversationOwnershipState,
    pub cause: ConversationDeletionCause,
    pub initiator_principal_id: PrincipalId,
    pub initiator_surface_id: String,
    pub correlation_id: String,
    pub provider_session_id: Option<ExecutionSessionId>,
    pub provider_erasure: ProviderSessionErasure,
    pub requested_at_ms: u64,
}

/// Why a conversation was deleted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationDeletionCause {
    /// A verified caller who owns it asked. Nothing deletes a conversation on
    /// its own.
    CallerRequested,
}

/// Commits deletion evidence before a conversation's history is erased.
///
/// Records are idempotent by conversation identity: one conversation is
/// deleted once, so the same record repeated is acknowledged again, and a
/// record contradicting the one stored is refused. An unavailable sink leaves
/// the history where it is.
pub trait ConversationDeletionAudit: Send + Sync {
    fn record(&self, record: ConversationDeletionAuditRecord) -> ConversationFuture<'_, ()>;
}
/// Waits for one-time runtime preparation to finish before this context opens a
/// provider on a request path.
///
/// The first launch of a newly installed runtime is scanned by the operating
/// system, and that cost belongs to whoever pays it first. When something else
/// is already paying it, a conversation joins that work instead of starting a
/// second cold launch of its own.
///
/// Waiting is all this promises. Preparation failing is not evidence that this
/// conversation's launch will fail, so there is no failure to return: the
/// conversation opens its provider and reports its own outcome either way.
pub trait RuntimeReadiness: Send + Sync {
    fn wait(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// Stores only conversation ownership, and the tombstone of one that was
/// deleted. The SDK stores execution history separately.
///
/// A conversation read back carries its tombstone when it has one, so a reader
/// does not see a deleted conversation as a live one: `load` returns it with
/// its [`Conversation::deletion`], and `create` for its identity
/// returns it as [`ConversationCreationDisposition::Existing`] rather than
/// creating it again
/// (`a_tombstone_outlives_reopening_and_its_identity_is_never_created_again`).
pub trait ConversationRepository: Send + Sync {
    /// The conversation as it is recorded, or `None` when there is no record
    /// of it. [`ConversationError::AgentUnsupported`] for a record written
    /// before records named their agent; [`ConversationError::Metadata`] for
    /// one that cannot be read.
    fn load(&self, id: &ConversationId) -> ConversationFuture<'_, Option<Conversation>>;
    /// Create once, or return the existing owner without changing it.
    fn create(&self, conversation: Conversation) -> ConversationFuture<'_, ConversationCreation>;
    /// Write the conversation's tombstone, or carry the one it has further,
    /// and return the conversation as it now stands.
    ///
    /// What is stored is what [`Conversation::deleted`] makes of the stored
    /// conversation and `deletion` — the domain's rule that the first decision
    /// stands; an implementation only persists it. Durable before it
    /// returns. [`ConversationError::NotFound`] when there is no ownership
    /// record to delete; [`ConversationError::AgentUnsupported`] for a record
    /// written before records named their agent, as [`Self::load`] answers
    /// it; [`ConversationError::Metadata`] for a record that cannot be read,
    /// a tombstone that could not stand beside it
    /// ([`Conversation::deleted`]), which is not written, or a write that
    /// could not be made durable. That last may already have landed — a
    /// tombstone published whose directory could not then be synced — so a
    /// caller cannot take any error here to mean nothing was written.
    fn record_deletion(
        &self,
        id: &ConversationId,
        deletion: ConversationDeletion,
    ) -> ConversationFuture<'_, Conversation>;
    /// Every conversation whose tombstone says its erasure has not finished,
    /// whoever owns it: the startup finish's question
    /// ([`super::ConversationService::finish_deletions`]).
    ///
    /// A tombstone whose conversation cannot even be named is left out and
    /// counted, since it may be one of them. Only being unable to ask at all
    /// is an error.
    fn unfinished_deletions(&self) -> ConversationFuture<'_, UnfinishedDeletions>;
}

/// What [`ConversationRepository::unfinished_deletions`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnfinishedDeletions {
    /// Each conversation whose tombstone says its erasure has not finished.
    pub conversations: Vec<ConversationId>,
    /// Tombstones of unfinished deletions whose conversation could not be
    /// named, so are not among [`Self::conversations`].
    pub unreadable: usize,
}

/// Answers what a list of one owner's conversations shows, from ownership,
/// tombstones and summaries together, reading only that owner's.
///
/// A port of its own because the answer spans [`ConversationRepository`] and
/// [`ConversationSummaries`]: a store that keeps both answers it in one
/// question, without reading anybody else's conversations
/// (docs/adr/todo/196-conversation-metadata-database.md).
pub trait ConversationListing: Send + Sync {
    /// The conversations `organization` and `owner` hold that are not deleted
    /// and have a summary whose archived flag is `archived`: most recently
    /// updated first, then by identity, at most `limit` of them.
    ///
    /// Whose a conversation is, is [`Conversation::allows`]'s answer, and a
    /// store gives the same one while reading only this owner's rows
    /// (`the_list_asks_whose_a_conversation_is_as_the_domain_answers_it`,
    /// `the_list_query_reads_only_its_owners_rows_however_many_others_there_are`).
    /// A row of this owner's that cannot be read back is left out and counted,
    /// since it may belong in the list. Only being unable to ask at all is an
    /// error.
    fn list(
        &self,
        organization: &OrganizationId,
        owner: &PrincipalId,
        archived: bool,
        limit: usize,
    ) -> ConversationFuture<'_, ListedConversations>;
}

/// What [`ConversationListing::list`] read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListedConversations {
    /// In the order listed.
    pub conversations: Vec<ListedConversation>,
    /// How many rows among the ones asked for could not be read, so are not
    /// among [`Self::conversations`].
    pub unreadable: usize,
}

/// One conversation in a list, with what the list shows about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedConversation {
    /// The conversation, as its ownership record reads.
    pub conversation: Conversation,
    /// Its title, last line said, time and archived flag.
    pub summary: ConversationSummary,
}

/// Keeps what a list of conversations shows about each one: its title, the
/// last thing said, and when.
///
/// Apart from [`ConversationRepository`] on purpose. Ownership is written once
/// and is evidence; a summary is rewritten on every turn and is a projection
/// of what was said. A conversation with no summary is still a conversation,
/// and a summary that could not be written leaves the list stale, never the
/// conversation broken.
pub trait ConversationSummaries: Send + Sync {
    fn load(&self, id: &ConversationId) -> ConversationFuture<'_, Option<ConversationSummary>>;
    /// Replace the conversation's summary with `summary`.
    fn record(
        &self,
        id: &ConversationId,
        summary: ConversationSummary,
    ) -> ConversationFuture<'_, ()>;
    /// Remove the conversation's summary, if it has one. Durable before it
    /// returns; a summary that is already gone is not an error.
    fn erase(&self, id: &ConversationId) -> ConversationFuture<'_, ()>;
}

/// An image as a caller submitted it, before any of it has been checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmittedImage {
    pub digest: String,
    pub media_type: String,
    pub size: u64,
}

/// One message as a caller submitted it, before any of it has been checked.
///
/// Grouped rather than passed one by one, for the same reason
/// `ConversationDependencies` is: the three parts of a message change together,
/// and a caller that forgets one should not compile.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubmittedMessage {
    pub text: String,
    /// Images already uploaded into this conversation, in attachment order.
    pub images: Vec<SubmittedImage>,
    /// Files on this machine the message points the agent at, in attachment
    /// order. Nothing was uploaded for these.
    pub files: Vec<SubmittedFile>,
}

/// A path as a caller submitted it, before any of it has been checked.
///
/// Unlike an image, nothing was uploaded for this and there is nothing for the
/// conversation to hold, so there is no ownership to verify: the whole of what
/// the gateway can check is whether the path can be said, which is the domain's
/// rule. See [`ConversationFileLinkAuditRecord`] for what it records instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmittedFile {
    pub path: String,
}

/// Immutable evidence that a verified caller submitted a message naming files
/// on this machine.
///
/// This is recorded because it is the only trace of it. An image is uploaded
/// first, so the attachments context already records who put it there; a path
/// is uploaded nowhere, and the reach it asks for — the agent may open these
/// files, anywhere on the disk, once the reader approves each read — would
/// otherwise exist only inside a prompt nobody keeps.
///
/// **This is intent, not a confirmed effect, and the field names say so.**
/// `before` and `after` are about the submission and nothing else: a message
/// that named no files now names these. Three things it deliberately does not
/// claim, in increasing order of how far away they are:
///
/// - that the message was admitted. The record is written first, because the
///   failure worth avoiding is an agent holding a path with no record of who
///   pointed it there. A submission refused after this — over its token budget,
///   or conflicting with one the agent already has — leaves a record that is
///   still true: somebody did submit it.
/// - that the agent received the files. It receives a link, as text.
/// - that anything was read. Every read is a separate tool call the reader
///   approves one at a time, and the agent may never make one.
///
/// Because it records a naming rather than a grant, there is nothing for a
/// later queue removal, cancellation, close, or restoration to transition: none
/// of those makes it untrue that the message named these files, and none of
/// them un-names them. Evidence of what the agent actually did with a link
/// belongs to the permission audit, which records each approved read.
///
/// There is no separate correlation identifier, and its absence is deliberate.
/// `execution_id` is what correlates this: the record is about one submission,
/// and that is the submission's name. The wire's `requestId` identifies one
/// *attempt* at it, and a submission can be attempted more than once — a send
/// whose dispatch failed comes back under the same execution identity and a
/// fresh request identity. Recording the attempt inside a record keyed by the
/// submission made those two disagree: the second attempt built a differing
/// document, the sink refused it as contradictory evidence, and the submission
/// became permanently unsendable while reporting a sink failure that had not
/// happened.
///
/// Nothing is lost by it. `conversation.send` carries `requestId` and
/// `executionId` together, so a stored record still joins to the requests that
/// produced it — through the submission they were all attempts at, rather than
/// through whichever attempt happened to be the one that wrote. That is the
/// more useful join anyway: the interesting question about a naming is which
/// message named it, and the interesting question about a *failed* attempt is
/// answered by the transport log, which has every request identity and this
/// never did.
///
/// [`ConversationCreationAuditRecord`] and [`AttachmentRelease`] do carry one,
/// and the asymmetry is the rule rather than an exception to it: a record may
/// name the request when the record is *of* that request, and may not when the
/// record is of something a caller may attempt more than once. Creation writes
/// a record per transition — an original and a later reopen are two records
/// with two causes — and a release happens once per close. This one is keyed by
/// a submission that is deliberately retryable, so the request identity is the
/// one field about it that is not stable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationFileLinkAuditRecord {
    pub conversation_id: ConversationId,
    pub organization_id: OrganizationId,
    /// The submission whose message names these files.
    pub execution_id: String,
    /// Every path, in attachment order, exactly as the message carries it.
    pub paths: Vec<String>,
    pub before: ConversationFileLinkState,
    pub after: ConversationFileLinkState,
    pub cause: ConversationFileLinkCause,
    pub initiator_principal_id: PrincipalId,
    pub initiator_surface_id: String,
    pub observed_at_ms: u64,
}

/// Whether a submission names files. Not whether the agent has them, and not
/// whether anything was opened; see [`ConversationFileLinkAuditRecord`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationFileLinkState {
    NotNamed,
    Named,
}

/// Why a submission came to name files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationFileLinkCause {
    /// A verified caller sent a message naming them. There is no automatic
    /// cause: nothing but a person's own submission puts a path in a prompt.
    CallerSubmitted,
}

/// Commits file-naming evidence before the submission is admitted.
///
/// Records are idempotent by conversation and submission identity, so a retry
/// of the same submission reconciles the same evidence rather than writing a
/// second record — and a submission claiming *different* paths under an
/// identity already recorded is a contradiction the sink refuses, which is what
/// keeps the trail from being made to name a file nobody sent. An unavailable
/// sink refuses the submission: an agent given a path with no record of who
/// pointed it there is the thing this prevents.
pub trait ConversationFileLinkAudit: Send + Sync {
    fn record(&self, record: ConversationFileLinkAuditRecord) -> ConversationFuture<'_, ()>;
}

/// Why a conversation let go of the files uploaded into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentReleaseCause {
    /// The verified caller closed the conversation.
    ConversationClosed,
    /// The verified caller deleted the conversation.
    ConversationDeleted,
}

/// A verified caller's release of one conversation's uploaded files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentRelease {
    pub organization_id: OrganizationId,
    pub conversation_id: ConversationId,
    pub cause: AttachmentReleaseCause,
    pub initiator_principal_id: PrincipalId,
    pub initiator_surface_id: String,
    pub correlation_id: String,
}

/// What a conversation needs from wherever uploaded files are kept.
///
/// A message refers to images by digest. Before accepting one, the service asks
/// whether *this* conversation holds exactly those bytes, so a digest learned
/// elsewhere cannot be used to read another owner's upload.
pub trait ConversationAttachments: Send + Sync {
    /// Whether the conversation holds an upload whose digest, media type, and
    /// size all agree with `image`.
    fn holds<'a>(
        &'a self,
        organization_id: &'a OrganizationId,
        conversation_id: &'a ConversationId,
        image: &'a ImageReference,
    ) -> ConversationFuture<'a, bool>;
    /// Let go of everything the conversation holds. The implementation records
    /// the release with its cause and initiator; an `Err` reports that some of
    /// it, or its evidence, could not be completed after every part was tried.
    fn release(&self, release: AttachmentRelease) -> ConversationFuture<'_, ()>;
}
