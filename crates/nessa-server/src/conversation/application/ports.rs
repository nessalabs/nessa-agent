use super::ConversationError;
use crate::conversation::domain::{Conversation, ConversationId};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::domain::agent_execution::prompts::ImageReference;
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

/// Stores only conversation ownership. The SDK stores execution history separately.
pub trait ConversationRepository: Send + Sync {
    fn load(&self, id: &ConversationId) -> ConversationFuture<'_, Option<Conversation>>;
    /// Create once, or return the existing owner without changing it.
    fn create(&self, conversation: Conversation) -> ConversationFuture<'_, ConversationCreation>;
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
