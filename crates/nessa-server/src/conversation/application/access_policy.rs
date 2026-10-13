//! The one question conversation access answers: may this principal take
//! this action on this conversation, or on this owner's catalogue, now?
//! (ADR 803 step 3, gate 18.)
//!
//! ```text
//! service / passive read / store transaction / attachments
//!   ──▶ AccessAsk{subject, action, resource, held, request, target}
//!   ──▶ ConversationAccessPolicyEvaluator::evaluate
//!       ──▶ Allow{policies} | Deny(Forbidden{policies} | NotPermitted | Unavailable)
//! OwnerAccess::admit(conversation, caller)
//!   ──▶ conversation.act ── Allow ──▶ Ok
//!                        └─ Deny ──▶ conversation.locate ── Allow ──▶ Deleted
//!                                                        └─ Deny ──▶ NotFound
//!   ──▶ Unavailable on either ──▶ Unavailable
//! ```
//!
//! Arrows are calls. The facts are what is true: who asks (an owner's own
//! surface, a paired reader with the owner its binding names, or a grant
//! holder), the conversation's organization, owner and whether it was
//! deleted, the role a grant holds, what a request carries, and what a share
//! target holds. The answer is the policy's: no role ordering, ownership
//! comparison or method set is decided here. Composition injects the Cedar
//! adapter (`infrastructure::access_policy::cedar`); an evaluator fails
//! closed, answering `Deny(Unavailable)` on any error of its own.
//!
//! The port is synchronous so the store can ask it inside the transaction
//! that writes a share grant, where a delete cannot land between the
//! question and the write.
use super::ConversationCaller;
use crate::conversation::domain::{Conversation, ConversationRefusal, ShareRole};
use nessa_auth::domain::{Grant, OrganizationId, PrincipalId, Resource};

/// Who is asking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessSubject<'a> {
    /// A person on one of their own surfaces, by the organization and
    /// principal their session is in.
    Person {
        organization: &'a OrganizationId,
        principal: &'a PrincipalId,
    },
    /// A paired party reading through its receiver binding: a device signs
    /// in as `binding_owner`, a peer gateway as itself.
    PairedReader {
        /// The organization the binding and session are in.
        organization: &'a OrganizationId,
        /// The principal the session signed in as.
        signed_in_as: &'a PrincipalId,
        /// The grantor whose conversations the binding reads.
        binding_owner: &'a PrincipalId,
        /// The receiver the binding names.
        receiver: &'a str,
    },
    /// The holder of a share grant, by its receiver, with no session: a note
    /// it left being delivered later.
    GrantHolder { receiver: &'a str },
}

/// What the subject asks to do. Each is one Cedar action of the same name
/// ([`Self::name`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AccessAction {
    /// Find the conversation, deleted or not: whether it is the subject's
    /// to be told about at all.
    Locate,
    /// Act on the conversation: every owner command on it.
    Act,
    /// List the owner's conversations, on the catalogue.
    List,
    /// Read the conversation.
    Read,
    /// See the conversation as an entry of the owner's catalogue, a
    /// tombstone included.
    CatalogueEntry,
    /// Leave a note in the conversation.
    Comment,
    /// Start, steer and stop turns in the conversation.
    Drive,
    /// Any other owner command on the conversation, asked by a paired
    /// reader.
    Write,
    /// Grant a paired party a share of the conversation.
    Share,
    /// Revoke a share of the conversation.
    Unshare,
    /// List the shares of the conversation.
    Shares,
    /// Watch the owner's catalogue for changes.
    CatalogueWatch,
    /// Read the bytes of a file the conversation holds.
    ArtifactRead,
    /// Deliver a note a grant holder left.
    NoteDeliver,
}

impl AccessAction {
    /// Every action, in declaration order.
    pub const ALL: [Self; 14] = [
        Self::Locate,
        Self::Act,
        Self::List,
        Self::Read,
        Self::CatalogueEntry,
        Self::Comment,
        Self::Drive,
        Self::Write,
        Self::Share,
        Self::Unshare,
        Self::Shares,
        Self::CatalogueWatch,
        Self::ArtifactRead,
        Self::NoteDeliver,
    ];

    /// The action's name, as the policy spells it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Locate => "conversation.locate",
            Self::Act => "conversation.act",
            Self::List => "conversation.list",
            Self::Read => "conversation.read",
            Self::CatalogueEntry => "catalogue.entry",
            Self::Comment => "conversation.comment",
            Self::Drive => "conversation.drive",
            Self::Write => "conversation.write",
            Self::Share => "conversation.share",
            Self::Unshare => "conversation.unshare",
            Self::Shares => "conversation.shares",
            Self::CatalogueWatch => "catalogue.watch",
            Self::ArtifactRead => "artifact.read",
            Self::NoteDeliver => "note.deliver",
        }
    }
}

/// What the subject asks about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessResource<'a> {
    /// One conversation.
    Conversation {
        organization: &'a OrganizationId,
        owner: &'a PrincipalId,
        /// Whether somebody deleted it.
        deleted: bool,
    },
    /// One owner's catalogue: the list of their conversations.
    Catalogue {
        organization: &'a OrganizationId,
        owner: &'a PrincipalId,
    },
}

impl<'a> AccessResource<'a> {
    /// `conversation` as the facts a policy decides on.
    pub fn of(conversation: &'a Conversation) -> Self {
        Self::Conversation {
            organization: conversation.organization(),
            owner: conversation.owner(),
            deleted: conversation.deletion().is_some(),
        }
    }
}

/// What a request carries, as far as a policy may care.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RequestShape<'a> {
    /// It names images from the owner's attachment store.
    pub attachments: bool,
    /// It links files by a path on the owner's machine.
    pub files: bool,
    /// For a receipt lookup, the operation whose receipt it asks for.
    pub receipt_operation: Option<&'a str>,
}

/// The party a share would grant to, as the receiver authority and the
/// credential registry answer for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShareTarget<'a> {
    /// The organization its receiver binding is in.
    pub organization: &'a OrganizationId,
    /// The owner its receiver binding names.
    pub binding_owner: &'a PrincipalId,
    /// What its credential holds.
    pub grants: &'a [Grant],
    /// This gateway, the resource its reads are asked on.
    pub gateway: &'a Resource,
}

/// One question for the access policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccessAsk<'a> {
    pub subject: AccessSubject<'a>,
    pub action: AccessAction,
    pub resource: AccessResource<'a>,
    /// The role of the share grant the subject holds on the resource, read
    /// now; `None` when it holds none.
    pub held: Option<ShareRole>,
    pub request: RequestShape<'a>,
    /// For a share, the party it grants to.
    pub target: Option<ShareTarget<'a>>,
}

impl<'a> AccessAsk<'a> {
    /// A person's `action` on `conversation`, with no grant, request or
    /// target to speak of.
    pub fn person(
        action: AccessAction,
        conversation: &'a Conversation,
        organization: &'a OrganizationId,
        principal: &'a PrincipalId,
    ) -> Self {
        Self {
            subject: AccessSubject::Person {
                organization,
                principal,
            },
            action,
            resource: AccessResource::of(conversation),
            held: None,
            request: RequestShape::default(),
            target: None,
        }
    }
}

/// The policy's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccessDecision {
    /// Admitted, by these policies.
    Allow { policies: Vec<String> },
    /// Refused, and why.
    Deny(AccessDenial),
}

/// Why access was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccessDenial {
    /// These policies forbid it.
    Forbidden { policies: Vec<String> },
    /// No policy allowed it.
    NotPermitted,
    /// The evaluator could not decide; it has logged why.
    Unavailable,
}

/// Whether a principal may access a conversation, decided by the access
/// policy.
pub trait ConversationAccessPolicyEvaluator: Send + Sync {
    /// The decision for `ask`. Never an error: an evaluator that cannot
    /// decide logs why and answers [`AccessDenial::Unavailable`].
    fn evaluate(&self, ask: &AccessAsk<'_>) -> AccessDecision;
}

/// Allowed, refused, or undecided, with a refusal's policies logged.
enum Answer {
    Allowed,
    Refused,
    Unavailable,
}

fn answer(policy: &dyn ConversationAccessPolicyEvaluator, ask: &AccessAsk<'_>) -> Answer {
    match policy.evaluate(ask) {
        AccessDecision::Allow { .. } => Answer::Allowed,
        AccessDecision::Deny(AccessDenial::Unavailable) => Answer::Unavailable,
        AccessDecision::Deny(denial) => {
            tracing::debug!(
                action = ask.action.name(),
                ?denial,
                "conversation access refused"
            );
            Answer::Refused
        }
    }
}

/// Whether a person may take an owner's action on a conversation, as the
/// refusal a caller is told: only someone the policy lets locate it learns
/// that it was deleted; to anybody else it is not found.
#[derive(Clone, Copy)]
pub struct OwnerAccess<'a> {
    policy: &'a dyn ConversationAccessPolicyEvaluator,
}

impl<'a> OwnerAccess<'a> {
    pub fn new(policy: &'a dyn ConversationAccessPolicyEvaluator) -> Self {
        Self { policy }
    }

    /// Whether `caller` may act on `conversation` (`conversation.act`).
    ///
    /// # Errors
    /// As [`Self::admit_to`].
    pub fn admit(
        &self,
        conversation: &Conversation,
        caller: &ConversationCaller,
    ) -> Result<(), ConversationRefusal> {
        self.admit_to(
            AccessAction::Act,
            conversation,
            &caller.organization_id,
            &caller.principal_id,
        )
    }

    /// Whether `principal` in `organization` may take `action` on
    /// `conversation`. Locating is asked only on a refusal, to say which.
    ///
    /// # Errors
    /// `Deleted` when the policy refuses `action` and lets the principal
    /// locate the conversation, `NotFound` when it refuses both, and
    /// `Unavailable` when it could not decide either.
    pub fn admit_to(
        &self,
        action: AccessAction,
        conversation: &Conversation,
        organization: &OrganizationId,
        principal: &PrincipalId,
    ) -> Result<(), ConversationRefusal> {
        match answer(
            self.policy,
            &AccessAsk::person(action, conversation, organization, principal),
        ) {
            Answer::Allowed => Ok(()),
            Answer::Unavailable => Err(ConversationRefusal::Unavailable),
            Answer::Refused => Err(if self.locates(conversation, organization, principal)? {
                ConversationRefusal::Deleted
            } else {
                ConversationRefusal::NotFound
            }),
        }
    }

    /// Whether `caller` may act on `conversation` (`conversation.act`),
    /// without asking which refusal it would be told: for a list, which
    /// leaves out what it may not act on.
    ///
    /// # Errors
    /// `Unavailable` when the policy could not decide.
    pub fn admits(
        &self,
        conversation: &Conversation,
        caller: &ConversationCaller,
    ) -> Result<bool, ConversationRefusal> {
        match answer(
            self.policy,
            &AccessAsk::person(
                AccessAction::Act,
                conversation,
                &caller.organization_id,
                &caller.principal_id,
            ),
        ) {
            Answer::Allowed => Ok(true),
            Answer::Refused => Ok(false),
            Answer::Unavailable => Err(ConversationRefusal::Unavailable),
        }
    }

    /// Whether `principal` in `organization` may be told about
    /// `conversation` at all, deleted or not (`conversation.locate`).
    ///
    /// # Errors
    /// `Unavailable` when the policy could not decide.
    pub fn locates(
        &self,
        conversation: &Conversation,
        organization: &OrganizationId,
        principal: &PrincipalId,
    ) -> Result<bool, ConversationRefusal> {
        match answer(
            self.policy,
            &AccessAsk::person(AccessAction::Locate, conversation, organization, principal),
        ) {
            Answer::Allowed => Ok(true),
            Answer::Refused => Ok(false),
            Answer::Unavailable => Err(ConversationRefusal::Unavailable),
        }
    }
}
