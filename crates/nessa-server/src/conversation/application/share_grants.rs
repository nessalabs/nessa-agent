//! Per-conversation share grants (issue 704, slice G of ADR 252; roles and
//! tool policies, issue 707): which paired party may do what in which of its
//! owner's conversations. `docs/design/read-grants.md` holds the rules and
//! the state table its tests are written from.
//!
//! A grant has a role, Read, Comment or Drive, and a Drive grant a tool
//! policy ([`ShareAccess`]). What each role lets its holder do is the
//! conversation access policy's (`ConversationAccessPolicyEvaluator`, ADR
//! 803 step 3): every role reads, and reads exactly as Read does (row J1);
//! only a Drive grant drives the conversation it names, and a Comment or
//! Drive grant leaves notes in it (rows J11 and J17). This module reads the
//! facts the policy is asked with, the conversation and the role its
//! receiver holds there ([`paired_facts`]), and never compares roles itself.
//!
//! Every read path a paired party reaches asks the policy, and none keeps
//! its own copy:
//!
//! - a paired device's record head, page and records watch
//!   (`AdmitPassiveRead::execute`);
//! - the socket's `conversation.read`, `artifact.read` and every view
//!   subscription batch (`product::read_access`). The socket's lists
//!   (`conversation.list`, `conversation.observe`, list subscriptions) are
//!   the owner's own and refuse a paired device, which lists through its
//!   catalogue.
//!
//! A paired device's catalogue pages are narrowed by the store in SQL, because
//! a filtered page must still be a whole page; that is an index, and each
//! entry it returns is admitted by the policy again (`catalogue.entry`).
//!
//! Who is a grantee is decided by construction, not by a list of credential
//! kinds: a session whose credential has a receiver binding was paired,
//! because only pairing creates receiver bindings, and it reads only what was
//! granted to that receiver. Every other session is the owner's own surface
//! and reads by ownership, exactly as before this slice.
//!
//! A paired party is a device, which signs in as its owner, or a peer
//! gateway, which signs in as its own `gateway` principal
//! (`docs/design/auth/peer-gateways.md`). Neither is compared with the
//! binding's owner: that is the grantor whose conversations it reads, and the
//! binding is matched to the session by credential and organization. Every
//! read path a paired party reaches asks the grant above, so a peer reads
//! exactly what a device would. The grantor's liveness rests on the pairing
//! initiator's membership staying active, which no runtime operation changes
//! today; see `AdmitPassiveRead::binding` for the change that must check it.

use super::metadata_target::load_conversation;
use super::{
    AccessAction, AccessAsk, AccessDecision, AccessDenial, AccessResource, AccessSubject,
    ConversationAccessPolicyEvaluator, ConversationCaller, ConversationError, ConversationFuture,
    ConversationRepository, OwnerAccess, PairedRefusal, RequestShape, ShareTarget,
};
use crate::conversation::application::ReceiverAuthority;
use crate::conversation::domain::{
    Conversation, ReceiverBinding, ShareAccess, ShareRole, ToolPolicy,
};
use nessa_auth::application::ports::{AccessError, AccessReader};
use nessa_auth::domain::{AuthContext, CredentialId, Resource};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::conversation::read_scope::ReadRefusal;
use nessa_sdk::application::agent_execution::sessions::{NoteCandidates, NoteRecord};
use nessa_sdk::domain::agent_execution::executions::Initiator;

/// Who is reading, as far as read grants are concerned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reader {
    /// One of the owner's own surfaces: reads what the owner owns.
    Owner,
    /// A paired device: reads only the conversations granted to it.
    PairedDevice { receiver_id: String },
}

impl Reader {
    /// The reader a session's receiver binding makes it. A binding exists
    /// only for a paired device, so no binding is the owner's own surface.
    pub fn of(binding: Option<&ReceiverBinding>) -> Self {
        match binding {
            Some(binding) => Self::PairedDevice {
                receiver_id: binding.receiver_id.clone(),
            },
            None => Self::Owner,
        }
    }
}

/// The active receiver binding `context`'s credential holds, read from the
/// receiver authority now, or `None` for the owner's own surface: a binding
/// exists only for a paired party, because only pairing makes one. A binding
/// no longer active is refused `Unauthorized`, as a passive read refuses it;
/// one that names another credential or organization is `Unverifiable`. The
/// binding's owner is the grantor, not the reader, so it is not compared
/// with the session's principal here: a peer gateway is its own principal,
/// and the access policy reads both.
pub async fn binding_of(
    receivers: &dyn ReceiverAuthority,
    context: &AuthContext,
) -> Result<Option<ReceiverBinding>, ReadRefusal> {
    let binding = receivers.resolve(context.credential_id()).await?;
    if let Some(binding) = &binding {
        if !binding.active {
            return Err(ReadRefusal::Unauthorized);
        }
        if binding.credential_id != *context.credential_id()
            || binding.organization_id != *context.organization_id()
        {
            return Err(ReadRefusal::Unverifiable);
        }
    }
    Ok(binding)
}

/// The facts a paired party's question about conversation `id` is asked
/// with, read now: the conversation, and the grant `receiver` holds there.
///
/// # Errors
/// `NotFound` when there is no such conversation, as for an ungranted one
/// (row G1); `Unavailable` when either could not be read.
pub async fn paired_facts(
    conversations: &dyn ConversationRepository,
    grants: &dyn ShareGrants,
    receiver: &str,
    id: &ConversationId,
) -> Result<(Conversation, Option<HeldGrant>), PairedRefusal> {
    let conversation = load_conversation(conversations, id)
        .await
        .map_err(|_| PairedRefusal::Unavailable)?
        .ok_or(PairedRefusal::NotFound)?;
    let held = grants
        .held_grant(id, receiver)
        .await
        .map_err(|_| PairedRefusal::Unavailable)?;
    Ok((conversation, held))
}

/// The host's verdict on the notes waiting in conversation `id` for its
/// next turn (ADR 0011 phase B, row J23): each `notes` entry pending
/// delivery is authorized, revoked, or left out. An owner's note is
/// authorized. A grant's note is asked of the access policy (`note.deliver`)
/// about the conversation as it is stored now, with the role its grant
/// holds now: allowed, authorized; refused (revoked, lowered below Comment,
/// or the conversation deleted), revoked, and the turn retires it; a
/// conversation or grant that could not be read, or a policy that could not
/// decide, left out, so the note waits for a turn that can check it. A
/// change of policy that keeps the role changes nothing: the note keeps the
/// revision it was left under, as its attribution.
///
/// The conversation is read only when a grant left a note, and each grant
/// is read and asked once, whatever number of notes it left.
pub async fn note_candidates(
    policy: &dyn ConversationAccessPolicyEvaluator,
    conversations: &dyn ConversationRepository,
    grants: &dyn ShareGrants,
    id: &ConversationId,
    notes: &[NoteRecord],
) -> NoteCandidates {
    let conversation = if notes.iter().any(|note| note.author != Initiator::Owner) {
        load_conversation(conversations, id).await.ok().flatten()
    } else {
        None
    };
    let mut candidates = NoteCandidates::default();
    let mut verdicts: Vec<(&str, Option<bool>)> = Vec::new();
    for note in notes {
        let allowed = match &note.author {
            Initiator::Owner => Some(true),
            Initiator::Grant(grant) => match verdicts
                .iter()
                .find(|(receiver, _)| *receiver == grant.grant())
            {
                Some((_, verdict)) => *verdict,
                None => {
                    let verdict = match &conversation {
                        Some(conversation) => {
                            deliverable(policy, grants, conversation, grant.grant()).await
                        }
                        None => None,
                    };
                    verdicts.push((grant.grant(), verdict));
                    verdict
                }
            },
        };
        match allowed {
            Some(true) => candidates.authorized.push(note.id.clone()),
            Some(false) => candidates.revoked.push(note.id.clone()),
            None => {}
        }
    }
    candidates
}

/// Whether a note `receiver`'s grant left in `conversation` may be
/// delivered now, or `None` when that could not be decided.
async fn deliverable(
    policy: &dyn ConversationAccessPolicyEvaluator,
    grants: &dyn ShareGrants,
    conversation: &Conversation,
    receiver: &str,
) -> Option<bool> {
    let held = grants.held_role(conversation.id(), receiver).await.ok()?;
    match policy.evaluate(&AccessAsk {
        subject: AccessSubject::GrantHolder { receiver },
        action: AccessAction::NoteDeliver,
        resource: AccessResource::of(conversation),
        held,
        request: RequestShape::default(),
        target: None,
    }) {
        AccessDecision::Allow { .. } => Some(true),
        AccessDecision::Deny(AccessDenial::Unavailable) => None,
        AccessDecision::Deny(_) => Some(false),
    }
}

/// The role a party holds on one conversation, and the revision of the
/// grant that set it, read together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeldGrant {
    pub role: ShareRole,
    /// As [`ShareGrant::policy_revision`].
    pub policy_revision: u64,
}

/// One grant as the owner sees it in `conversation.shares`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareGrant {
    pub conversation_id: ConversationId,
    pub receiver_id: String,
    pub credential_id: CredentialId,
    pub access: ShareAccess,
    /// The journal sequence of the change that last set the role or the
    /// policy, which every turn a Drive grant starts is saved with.
    pub policy_revision: u64,
    pub granted_at_ms: u64,
}

/// Grant with a role and policy, or revoke, as the owner decided it. A grant
/// on a party that already holds one replaces its role and policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShareGrantTransition {
    Grant(ShareAccess),
    Revoke,
}

impl ShareGrantTransition {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Grant(_) => "granted",
            Self::Revoke => "revoked",
        }
    }
}

/// The most grants one conversation holds, so its `conversation.shares`
/// answer always fits one socket frame. A grant past it is refused
/// `InvalidInput` (`docs/limits.md`).
pub const MAX_SHARE_GRANTS_PER_CONVERSATION: i64 = 64;

/// The most Drive grants one conversation holds. A Drive grant carries a tool
/// policy, and this bound is what keeps a `conversation.shares` answer of
/// [`MAX_SHARE_GRANTS_PER_CONVERSATION`] grants inside one frame with every
/// policy at its largest (rows J9, J10). A grant or change past it is refused
/// `InvalidInput` (`docs/limits.md`).
pub const MAX_DRIVE_GRANTS_PER_CONVERSATION: i64 = 8;

/// One owner decision about one paired party and one conversation, with who
/// made it. The store writes the grant, its journal row and the catalogue
/// revision in one transaction.
#[derive(Clone)]
pub struct ShareGrantChange {
    pub transition: ShareGrantTransition,
    pub conversation_id: ConversationId,
    /// Absent only for a revoke of a device whose binding is gone: the grant
    /// is then found by its credential.
    pub receiver_id: Option<String>,
    pub credential_id: CredentialId,
    pub initiator: ConversationCaller,
    pub at_ms: u64,
}

/// The gateway's table of share grants. Grants and follow rules are separate
/// tables: this is the owner's side only, and a follower's choice of what to
/// keep never reaches it.
pub trait ShareGrants: Send + Sync {
    /// The grant `receiver_id` holds on `id`, or `None` when it holds none
    /// there.
    fn held_grant<'a>(
        &'a self,
        id: &'a ConversationId,
        receiver_id: &'a str,
    ) -> ConversationFuture<'a, Option<HeldGrant>>;
    /// The role `receiver_id` holds on `id`, or `None` when it holds no
    /// grant there.
    fn held_role<'a>(
        &'a self,
        id: &'a ConversationId,
        receiver_id: &'a str,
    ) -> ConversationFuture<'a, Option<ShareRole>> {
        Box::pin(async move {
            Ok(self
                .held_grant(id, receiver_id)
                .await?
                .map(|held| held.role))
        })
    }
    /// Whether `receiver_id` holds a grant on `id`, of any role.
    fn is_granted<'a>(
        &'a self,
        id: &'a ConversationId,
        receiver_id: &'a str,
    ) -> ConversationFuture<'a, bool> {
        Box::pin(async move { Ok(self.held_role(id, receiver_id).await?.is_some()) })
    }
    /// Apply one change. `false` when it changed nothing (granting the role
    /// and policy already held, revoking a grant not held). A grant naming
    /// another role or policy than the one held replaces them and is `true`
    /// (row J4). Every admitted request is journaled under its request id, a
    /// no-op too (before equal to after), and a retry of the same request
    /// answers from that row and changes nothing (rows G9, G16, J5, J6). A
    /// retry is admitted like a new request first: ownership for both, and
    /// not deleted for a grant.
    fn change(&self, change: ShareGrantChange) -> ConversationFuture<'_, bool>;
    /// The grants on one conversation, oldest first.
    fn grants<'a>(&'a self, id: &'a ConversationId) -> ConversationFuture<'a, Vec<ShareGrant>>;
}

/// The owner's share commands: `conversation.share`, `.unshare`, `.shares`.
pub struct ShareConversation<'a> {
    pub conversations: &'a dyn ConversationRepository,
    pub receivers: &'a dyn ReceiverAuthority,
    pub grants: &'a dyn ShareGrants,
    /// The credential registry, read for the principal a share target names.
    pub access: &'a dyn AccessReader,
    /// This gateway, the resource a share target's read must be on.
    pub gateway: &'a Resource,
    /// The access policy, asked whether the caller may share, and with whom.
    pub policy: &'a dyn ConversationAccessPolicyEvaluator,
}

impl ShareConversation<'_> {
    /// Grant `credential`'s receiver `role` on `id` with `policy`, or replace
    /// the role and policy it holds there. A Drive grant needs a policy and a
    /// Read or Comment grant takes none; either mistake is `InvalidInput` and
    /// writes nothing (row J3). The credential must have an active receiver
    /// binding and a registry answer about that exact credential, or
    /// `ShareTargetNotPaired`. Whether the caller may share this
    /// conversation with it is the access policy's (`conversation.share`):
    /// the conversation the caller's and not deleted, the party paired by
    /// the caller in its organization, holding `conversation.read` on this
    /// gateway (an environment's credential serves and can never read, so a
    /// share to it would record a reader that is not one). Ownership is
    /// asked first (`conversation.act`), before the credential is looked
    /// at, so a caller that may not act on `id` is told what acting on it
    /// would tell it (`NotFound`, `Deleted`) whatever credential it names;
    /// one that may is told `ShareTargetNotPaired` for a party the policy
    /// refuses. The store asks ownership again in the transaction that
    /// writes the grant (row G8). Either kind of party reads only what it is
    /// granted (`docs/design/auth/peer-gateways.md`, row H8).
    pub async fn share(
        &self,
        caller: ConversationCaller,
        id: ConversationId,
        credential: CredentialId,
        role: ShareRole,
        policy: Option<ToolPolicy>,
        at_ms: u64,
    ) -> Result<bool, ConversationError> {
        caller.actor()?;
        let access = ShareAccess::new(role, policy).map_err(|_| ConversationError::InvalidInput)?;
        // The caller's own conversation is asked about first, so what it is
        // told about an id it may not act on (`NotFound`, `Deleted`) never
        // depends on the credential it names.
        let conversation = load_conversation(self.conversations, &id)
            .await?
            .ok_or(ConversationError::NotFound)?;
        OwnerAccess::new(self.policy).admit(&conversation, &caller)?;
        let binding = self
            .receivers
            .resolve(&credential)
            .await
            .map_err(|_| ConversationError::Metadata)?
            .filter(|binding| binding.active && binding.credential_id == credential)
            .ok_or(ConversationError::ShareTargetNotPaired)?;
        let target = match self.access.read(&credential).await {
            Ok(snapshot) => snapshot,
            Err(AccessError::Unavailable | AccessError::StaleRevision) => {
                return Err(ConversationError::Metadata)
            }
            Err(_) => return Err(ConversationError::ShareTargetNotPaired),
        };
        // The answer must be about the credential asked for, in the
        // binding's organization, with a membership of the same principal:
        // a grant is made on a registry answer about that exact credential.
        let named = &target.credential;
        if named.id() != &credential
            || named.organization_id() != &binding.organization_id
            || named.principal_id() != target.membership.principal_id()
            || named.organization_id() != target.membership.organization_id()
        {
            return Err(ConversationError::ShareTargetNotPaired);
        }
        let ask = AccessAsk {
            subject: AccessSubject::Person {
                organization: &caller.organization_id,
                principal: &caller.principal_id,
            },
            action: AccessAction::Share,
            resource: AccessResource::of(&conversation),
            held: None,
            request: RequestShape::default(),
            target: Some(ShareTarget {
                organization: &binding.organization_id,
                binding_owner: &binding.owner_id,
                grants: named.grants(),
                gateway: self.gateway,
            }),
        };
        match self.policy.evaluate(&ask) {
            AccessDecision::Allow { .. } => {}
            AccessDecision::Deny(AccessDenial::Unavailable) => {
                return Err(ConversationError::Metadata)
            }
            // The caller may act on the conversation, so the refusal is
            // for the party it names.
            AccessDecision::Deny(denial) => {
                tracing::debug!(?denial, "a share was refused");
                return Err(ConversationError::ShareTargetNotPaired);
            }
        }
        self.grants
            .change(ShareGrantChange {
                transition: ShareGrantTransition::Grant(access),
                conversation_id: id,
                receiver_id: Some(binding.receiver_id),
                credential_id: credential,
                initiator: caller,
                at_ms,
            })
            .await
    }

    /// Revoke `credential`'s grant on `id`, whatever its role. Allowed on a deleted conversation
    /// and for a device since unpaired: taking access away needs no more
    /// than ownership, which the store checks as it writes.
    pub async fn unshare(
        &self,
        caller: ConversationCaller,
        id: ConversationId,
        credential: CredentialId,
        at_ms: u64,
    ) -> Result<bool, ConversationError> {
        caller.actor()?;
        self.grants
            .change(ShareGrantChange {
                transition: ShareGrantTransition::Revoke,
                conversation_id: id,
                receiver_id: None,
                credential_id: credential,
                initiator: caller,
                at_ms,
            })
            .await
    }

    /// The grants on `id`, for its owner.
    pub async fn shares(
        &self,
        caller: &ConversationCaller,
        id: &ConversationId,
    ) -> Result<Vec<ShareGrant>, ConversationError> {
        self.owner_of(caller, id).await?;
        self.grants.grants(id).await
    }

    /// The caller may list `id`'s shares, deleted or not
    /// (`conversation.shares`).
    async fn owner_of(
        &self,
        caller: &ConversationCaller,
        id: &ConversationId,
    ) -> Result<(), ConversationError> {
        caller.actor()?;
        let conversation = load_conversation(self.conversations, id)
            .await?
            .ok_or(ConversationError::NotFound)?;
        OwnerAccess::new(self.policy).admit_to(
            AccessAction::Shares,
            &conversation,
            &caller.organization_id,
            &caller.principal_id,
        )?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/note_candidates.rs"]
mod note_candidate_tests;
