//! Per-conversation share grants (issue 704, slice G of ADR 252; roles and
//! tool policies, issue 707): which paired party may do what in which of its
//! owner's conversations. `docs/design/read-grants.md` holds the rules and
//! the state table its tests are written from.
//!
//! A grant has a capability set, read and for a device also comment and
//! drive, and a drive grant its constraints ([`ShareAccess`]): the tool
//! policy and command hosts. What each capability lets its holder do is the
//! conversation access policy's (`ConversationAccessPolicyEvaluator`): a
//! grant that includes read reads, and reads exactly as read alone does
//! (row J1); only a grant that includes drive drives the conversation it
//! names, and one that includes comment leaves notes in it (rows J11 and
//! J17). Comment and drive also require a principal signed in as the
//! binding owner, which is the device. This module reads the facts the
//! policy is asked with, the conversation and the capabilities its receiver
//! holds there ([`paired_facts`]), and never compares capabilities itself.
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
    ConversationRepository, FenceCapture, OwnerAccess, PairedRefusal, RequestShape, ShareTarget,
};
use crate::conversation::application::ReceiverAuthority;
use crate::conversation::domain::{
    Capabilities, Capability, Conversation, ReceiverBinding, ShareAccess, ToolPolicy,
};
use nessa_auth::application::ports::{AccessError, AccessHold, AccessReader};
use nessa_auth::domain::{AuthContext, CredentialId, PrincipalId, Resource};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::conversation::read_scope::ReadRefusal;
use nessa_sdk::application::agent_execution::sessions::{NoteCandidates, NoteRecord};
use nessa_sdk::domain::agent_execution::executions::Initiator;

/// Who is reading, as far as read grants are concerned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reader {
    /// One of the owner's own surfaces: reads what the owner owns.
    Owner,
    /// First-party purpose with current original holding; target policy is
    /// independent of per-conversation share grants and is never owner fallback.
    OwnerPhone(Box<super::OwnerPhoneReader>),
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
/// about the conversation as it is stored now, with the capabilities its
/// grant holds now: allowed, authorized; refused (revoked, lowered below comment,
/// or the conversation deleted), revoked, and the turn retires it; a
/// conversation or grant that could not be read, or a policy that could not
/// decide, left out, so the note waits for a turn that can check it. A
/// change of constraints that keeps comment changes nothing: the note keeps the
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
    let held = grants
        .held_capabilities(conversation.id(), receiver)
        .await
        .ok()?;
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

/// The capabilities a party holds on one conversation, and the grant epoch
/// that set them, read together. The epoch is [`ShareGrant::policy_revision`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeldGrant {
    pub capabilities: Capabilities,
    /// As [`ShareGrant::policy_revision`]: the grant epoch.
    pub policy_revision: u64,
}

/// The resource a grant names. This slice writes [`Self::Conversation`] and
/// the conversation id. A directory grant uses [`Self::Directory`] under the
/// same primary key; this slice does not write one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    Conversation,
    Directory,
}

impl ResourceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Conversation => "conversation",
            Self::Directory => "directory",
        }
    }
}

/// One grant as the owner sees it in `conversation.shares`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareGrant {
    pub conversation_id: ConversationId,
    /// The grantee. The row's key, with the resource kind and resource id.
    pub principal_id: PrincipalId,
    /// The receiver a paired device or gateway is bound to. Absent for an
    /// integration, and for a later agent, which store no receiver.
    pub receiver_id: Option<String>,
    pub credential_id: CredentialId,
    pub access: ShareAccess,
    /// The grant epoch: the journal sequence of the change that last set
    /// the capabilities or the constraints, which every turn a drive grant
    /// starts is saved with.
    pub policy_revision: u64,
    pub granted_at_ms: u64,
}

/// The grants on one conversation, and the catalogue revision they were read
/// with. A grant, a revoke, or a replacement of the receiver or credential
/// moves `change_revision`. A change of capabilities or constraints, for the
/// same receiver and credential, does not. `conversation.shares` names it on
/// every page, so a later page belongs to the same read only when it names
/// the same revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareGrantList {
    pub grants: Vec<ShareGrant>,
    pub change_revision: u64,
}

/// Grant with capabilities and constraints, or revoke, as the owner decided
/// it. A grant on a party that already holds one replaces them.
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

/// The largest policy revision the protocol carries, a JSON safe integer,
/// so every client reads it back exactly. The schema states it as the
/// `maximum` of `expectedPolicyRevision` and of `policyRevision`
/// (`tests/conversation/agreement.rs`); an expected revision past it is
/// refused `InvalidInput` (row J30).
pub const MAX_POLICY_REVISION: u64 = 9_007_199_254_740_991;

/// The most grants one conversation holds. A grant past it is refused
/// `InvalidInput`. `conversation.shares` returns them a page at a time, so
/// one answer does not have to hold all of them (`docs/limits.md`).
pub const MAX_SHARE_GRANTS_PER_CONVERSATION: i64 = 64;

/// The most Drive grants one conversation holds. A grant or change past it
/// is refused `InvalidInput` (row J9, `docs/limits.md`). One
/// `conversation.shares` page fits the frame because of its page size, not
/// because of this bound (row J10).
pub const MAX_DRIVE_GRANTS_PER_CONVERSATION: i64 = 8;

/// One owner decision about one paired party and one conversation, with who
/// made it. The store writes the grant, its journal row and the catalogue
/// revision in one transaction.
#[derive(Clone)]
pub struct ShareGrantChange {
    pub transition: ShareGrantTransition,
    pub conversation_id: ConversationId,
    /// The grantee. Required for a grant. A revoke may omit it and is then
    /// found by its credential.
    pub principal_id: Option<PrincipalId>,
    /// The receiver a paired device or gateway is bound to. Absent for a
    /// grant that stores none, and for a revoke of a device whose binding
    /// is gone: that revoke is found by its credential.
    pub receiver_id: Option<String>,
    pub credential_id: CredentialId,
    /// The policy revision the caller read the party's grant at, when it
    /// built this change on it. The store makes the change only while the
    /// grant is still at that revision, in the transaction that writes it,
    /// and refuses it `ShareGrantChanged` otherwise (rows J29, J30). `None`
    /// makes the change whatever the party holds.
    pub expected_policy_revision: Option<u64>,
    pub initiator: ConversationCaller,
    pub at_ms: u64,
}

/// What one change answered: whether it changed anything, and the policy
/// revision the party's grant holds once it was made. A grant always has
/// one: the new revision when applied, the one it kept when not. A revoke
/// leaves no grant, so it names none; a later grant takes a revision of its
/// own, past every one before it. A retried request answers what its first
/// answer said (row J29).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use]
pub struct ShareGrantReceipt {
    pub applied: bool,
    pub policy_revision: Option<u64>,
}

/// The gateway's table of share grants. Grants and follow rules are separate
/// tables: this is the owner's side only, and a follower's choice of what to
/// keep never reaches it.
pub trait ShareGrants: Send + Sync {
    /// Read a principal-subject grant without manufacturing a receiver binding.
    /// The credential remains part of the row and must be correlated by callers.
    fn principal_grant<'a>(
        &'a self,
        id: &'a ConversationId,
        principal: &'a PrincipalId,
    ) -> ConversationFuture<'a, Option<ShareGrant>> {
        Box::pin(async move {
            Ok(self
                .grants(id)
                .await?
                .grants
                .into_iter()
                .find(|grant| &grant.principal_id == principal))
        })
    }

    /// Fence this principal/resource epoch using the same gate as grant changes.
    /// An adapter without that synchronization fails closed.
    fn hold_principal_revision<'a>(
        &'a self,
        _id: &'a ConversationId,
        _principal: &'a PrincipalId,
        _policy_revision: u64,
    ) -> ConversationFuture<'a, RevisionHold> {
        Box::pin(async { Err(ConversationError::Metadata) })
    }

    /// The grant `receiver_id` holds on `id`, or `None` when it holds none
    /// there.
    fn held_grant<'a>(
        &'a self,
        id: &'a ConversationId,
        receiver_id: &'a str,
    ) -> ConversationFuture<'a, Option<HeldGrant>>;
    /// The capabilities `receiver_id` holds on `id`, or `None` when it holds
    /// no grant there.
    fn held_capabilities<'a>(
        &'a self,
        id: &'a ConversationId,
        receiver_id: &'a str,
    ) -> ConversationFuture<'a, Option<Capabilities>> {
        Box::pin(async move {
            Ok(self
                .held_grant(id, receiver_id)
                .await?
                .map(|held| held.capabilities))
        })
    }
    /// Whether `receiver_id` holds a grant on `id`.
    fn is_granted<'a>(
        &'a self,
        id: &'a ConversationId,
        receiver_id: &'a str,
    ) -> ConversationFuture<'a, bool> {
        Box::pin(async move { Ok(self.held_capabilities(id, receiver_id).await?.is_some()) })
    }
    /// Apply one change. Not `applied` when it changed nothing (granting the
    /// capabilities and constraints already held, revoking a grant not held). A grant
    /// naming other capabilities or constraints than the one held replaces them and is
    /// `applied` (row J4). Every admitted request is journaled under its
    /// request id, a no-op too (before equal to after), and a retry of the
    /// same request answers from that row and changes nothing (rows G9, G16,
    /// J5, J6), before any expected revision is compared. A retry is
    /// admitted like a new request first: ownership for both, and not
    /// deleted for a grant. A change naming an expected policy revision the
    /// grant no longer holds is refused `ShareGrantChanged` and writes
    /// nothing (row J30).
    fn change(&self, change: ShareGrantChange) -> ConversationFuture<'_, ShareGrantReceipt>;
    /// Retain the original Auth hold in every physical grant write, including
    /// after caller cancellation. This is ownership, not policy admission.
    /// An adapter that cannot transfer the hold to its writer refuses the call.
    fn change_with_credential(
        &self,
        _change: ShareGrantChange,
        _credential: AccessHold,
    ) -> ConversationFuture<'_, ShareGrantReceipt> {
        Box::pin(async { Err(ConversationError::Metadata) })
    }

    /// The grants on one conversation, oldest first, with the catalogue
    /// revision they were read at.
    fn grants<'a>(&'a self, id: &'a ConversationId) -> ConversationFuture<'a, ShareGrantList>;
    /// The tool policy `receiver_id`'s grant on `id` held at
    /// `policy_revision`, as the journal entry of that revision says; `None`
    /// when that entry is no Drive grant of that party on that conversation.
    /// The receiver the grant moved to reads the epoch the live row still
    /// names. An entry is never changed, so the answer for one revision is
    /// always the same, whatever the grant holds now.
    fn policy_at<'a>(
        &'a self,
        id: &'a ConversationId,
        receiver_id: &'a str,
        policy_revision: u64,
    ) -> ConversationFuture<'a, Option<ToolPolicy>>;
    /// Keep `receiver_id`'s grant on `id` at `policy_revision` until the
    /// returned hold is dropped. [`ConversationError::Revoked`] when that
    /// grant is gone, stands at another revision, or its revision cannot be
    /// read as one. A store that cannot be asked is [`ConversationError::Metadata`].
    ///
    /// A table that records no live grant returns [`RevisionHold::vacant`]:
    /// there is nothing to revoke. The metadata store's hold is that one
    /// grant, not every conversation, and a Drive turn proceeds only while
    /// it still finds the admitted revision (row J31).
    fn hold_revision<'a>(
        &'a self,
        id: &'a ConversationId,
        receiver_id: &'a str,
        policy_revision: u64,
    ) -> ConversationFuture<'a, RevisionHold>;
}

/// A grant revision kept still until this is dropped.
///
/// [`RevisionHold::vacant`] keeps nothing. The metadata store's hold keeps
/// that one grant's admission permit, so a change of the same grant waits
/// until the permit is dropped. Another conversation's grant is not waiting
/// on it.
#[must_use = "dropping the hold lets a grant change land before the turn is saved"]
pub struct RevisionHold {
    _keep: Option<Box<dyn Send>>,
}

impl RevisionHold {
    /// A hold that keeps nothing.
    pub fn vacant() -> Self {
        Self { _keep: None }
    }

    pub(crate) fn held(guard: impl Send + 'static) -> Self {
        Self {
            _keep: Some(Box::new(guard)),
        }
    }
}

/// What `conversation.share` answers: whether it changed anything, and the
/// grant's policy revision once it was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GrantedShare {
    pub applied: bool,
    pub policy_revision: u64,
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
    /// The deletion inventory, reconciled after a successful share and after
    /// any unshare. Absent, the grant is still written and the inventory is
    /// unchanged. A capture that fails after a revoke is logged and the
    /// revoke stays (`a_failed_inventory_capture_leaves_the_revoke`).
    pub inventory: Option<&'a dyn FenceCapture>,
}

impl ShareConversation<'_> {
    /// Grant `credential`'s receiver `capabilities` on `id` with `policy`, or
    /// replace the capabilities and constraints it holds there. A drive grant
    /// needs a policy and any other grant takes none; either mistake is
    /// `InvalidInput` and writes nothing (row J3). Comment and drive are
    /// asked of Cedar (`grant.comment`, `grant.drive`) before they are
    /// written. A denial writes nothing and carries the policy ids Cedar
    /// named. The use case does not match a gateway principal on its own.
    /// The credential must have an active receiver
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
    ///
    /// Answers the grant's policy revision once the share is made, which a
    /// later change to it names as `expected_policy_revision` (row J29).
    #[allow(clippy::too_many_arguments)]
    pub async fn share(
        &self,
        caller: ConversationCaller,
        id: ConversationId,
        credential: CredentialId,
        capabilities: Capabilities,
        policy: Option<ToolPolicy>,
        expected_policy_revision: Option<u64>,
        at_ms: u64,
    ) -> Result<GrantedShare, ConversationError> {
        caller.actor()?;
        let access =
            ShareAccess::new(capabilities, policy).map_err(|_| ConversationError::InvalidInput)?;
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
        // Comment and drive are written only when those permits allow them
        // for this principal. The facts are the principal the credential
        // names and the owner the binding reads; the principal's kind is
        // not a condition here.
        if access.capabilities().contains(Capability::Comment) {
            let mut policies = Vec::new();
            let mut denied = false;
            let actions = [AccessAction::Comment, AccessAction::Drive];
            for action in actions {
                if action == AccessAction::Drive
                    && !access.capabilities().contains(Capability::Drive)
                {
                    continue;
                }
                let grant_ask = AccessAsk {
                    subject: AccessSubject::PairedReader {
                        organization: &binding.organization_id,
                        signed_in_as: named.principal_id(),
                        binding_owner: &binding.owner_id,
                        receiver: &binding.receiver_id,
                    },
                    action,
                    resource: AccessResource::of(&conversation),
                    held: Some(access.capabilities()),
                    request: RequestShape::default(),
                    target: None,
                };
                match self.policy.evaluate(&grant_ask) {
                    AccessDecision::Allow { .. } => {}
                    AccessDecision::Deny(AccessDenial::Unavailable) => {
                        return Err(ConversationError::Metadata)
                    }
                    AccessDecision::Deny(AccessDenial::Forbidden { policies: ids }) => {
                        denied = true;
                        policies.extend(ids);
                    }
                    AccessDecision::Deny(AccessDenial::NotPermitted) => denied = true,
                }
            }
            if denied {
                policies.sort();
                policies.dedup();
                return Err(ConversationError::CapabilitiesRefused { policies });
            }
        }
        let receipt = self
            .grants
            .change(ShareGrantChange {
                transition: ShareGrantTransition::Grant(access),
                conversation_id: id,
                principal_id: Some(named.principal_id().clone()),
                receiver_id: Some(binding.receiver_id),
                credential_id: credential,
                expected_policy_revision,
                initiator: caller,
                at_ms,
            })
            .await?;
        // The grant is already committed. Capture reconciles the deletion
        // inventory, including a receiver the grant moved off. A failure is
        // logged and does not undo the grant. An equivalent no-op request
        // retries capture too, so a failed capture can be reconciled
        // (`a_retried_share_reconciles_after_a_failed_capture`).
        if let Some(inventory) = self.inventory {
            if let Err(error) = inventory.capture().await {
                tracing::warn!(
                    ?error,
                    "the deletion inventory was not updated after a share"
                );
            }
        }
        // A grant always holds a revision once it is made; a store that
        // answers none for one has broken its own contract.
        let Some(policy_revision) = receipt.policy_revision else {
            tracing::error!("the share grant store answered a grant with no policy revision");
            return Err(ConversationError::Metadata);
        };
        Ok(GrantedShare {
            applied: receipt.applied,
            policy_revision,
        })
    }

    /// Revoke `credential`'s grant on `id`, whatever its role. Allowed on a deleted conversation
    /// and for a device since unpaired: taking access away needs no more
    /// than ownership, which the store checks as it writes. `true` when a
    /// grant was taken away; either way the party holds none afterwards.
    pub async fn unshare(
        &self,
        caller: ConversationCaller,
        id: ConversationId,
        credential: CredentialId,
        expected_policy_revision: Option<u64>,
        at_ms: u64,
    ) -> Result<bool, ConversationError> {
        caller.actor()?;
        let applied = self
            .grants
            .change(ShareGrantChange {
                transition: ShareGrantTransition::Revoke,
                conversation_id: id,
                principal_id: None,
                receiver_id: None,
                credential_id: credential,
                expected_policy_revision,
                initiator: caller,
                at_ms,
            })
            .await?
            .applied;
        // A retry reconciles too: the revoke fence is already committed, and
        // a later re-grant drops it when this read no longer names it.
        if let Some(inventory) = self.inventory {
            if let Err(error) = inventory.capture().await {
                tracing::warn!(
                    ?error,
                    "the deletion inventory was not updated after an unshare"
                );
            }
        }
        Ok(applied)
    }

    /// The grants on `id`, for its owner.
    pub async fn shares(
        &self,
        caller: &ConversationCaller,
        id: &ConversationId,
    ) -> Result<ShareGrantList, ConversationError> {
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
