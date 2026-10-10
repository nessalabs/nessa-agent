//! Per-conversation read grants (issue 704, slice G of ADR 252): which paired
//! device may read which of its owner's conversations.
//! `docs/design/read-grants.md` holds the rules and the state table its tests
//! are written from.
//!
//! One authority answers "may this reader read this conversation":
//! [`admit_read`] for one conversation, and for a paired device's list the
//! store's grant predicate. Every read path asks one of them, and none keeps
//! its own copy:
//!
//! - a paired device's record head, page and records watch
//!   (`AdmitPassiveRead::execute`);
//! - the socket's `conversation.read` and every view subscription batch
//!   (`product::read_access`). The socket's lists (`conversation.list`,
//!   `conversation.observe`, list subscriptions) are the owner's own and
//!   refuse a paired device, which lists through its catalogue.
//!
//! A paired device's catalogue pages are narrowed by the store in SQL, because
//! a filtered page must still be a whole page; the store answers
//! [`ReadGrants::is_granted`] and those pages with one predicate
//! (`store::read_grants::granted`), so the two cannot disagree.
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
use super::{ConversationCaller, ConversationError, ConversationFuture, ConversationRepository};
use crate::conversation::application::ReceiverAuthority;
use crate::conversation::domain::ReceiverBinding;
use nessa_auth::application::ports::{AccessError, AccessReader};
use nessa_auth::domain::{AuthContext, CredentialId};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::conversation::read_scope::ReadRefusal;

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

/// The reader `context`'s credential is, read from the receiver authority
/// now. A paired party whose binding is no longer active is refused
/// `Unauthorized`, as a passive read refuses it; a binding that names another
/// credential or organization is `Unverifiable`. The binding's owner is the
/// grantor, not the reader, so it is not compared with the session's
/// principal: a peer gateway is its own principal and reads by grant alone.
pub async fn reader_of(
    receivers: &dyn ReceiverAuthority,
    context: &AuthContext,
) -> Result<Reader, ReadRefusal> {
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
    Ok(Reader::of(binding.as_ref()))
}

/// Whether `reader` may read conversation `id`. Ownership is the caller's
/// check, made where the conversation is loaded; this answers only the grant.
/// An ungranted id is refused `WrongOwner`, the answer a conversation that is
/// not the owner's gets, so a device cannot tell an ungranted id from one that
/// does not exist (row G1).
pub async fn admit_read(
    grants: &dyn ReadGrants,
    reader: &Reader,
    id: &ConversationId,
) -> Result<(), ReadRefusal> {
    match reader {
        Reader::Owner => Ok(()),
        Reader::PairedDevice { receiver_id } => match grants.is_granted(id, receiver_id).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(ReadRefusal::WrongOwner),
            Err(_) => Err(ReadRefusal::Unverifiable),
        },
    }
}

/// One grant as the owner sees it in `conversation.shares`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadGrant {
    pub conversation_id: ConversationId,
    pub receiver_id: String,
    pub credential_id: CredentialId,
    pub granted_at_ms: u64,
}

/// Grant or revoke, as the owner decided it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadGrantTransition {
    Grant,
    Revoke,
}

impl ReadGrantTransition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Grant => "granted",
            Self::Revoke => "revoked",
        }
    }
}

/// The most grants one conversation holds, so its `conversation.shares`
/// answer always fits one socket frame. A grant past it is refused
/// `InvalidInput` (`docs/limits.md`).
pub const MAX_READ_GRANTS_PER_CONVERSATION: i64 = 64;

/// One owner decision about one device and one conversation, with who made
/// it. The store writes the grant, its journal row and the catalogue
/// revision in one transaction.
#[derive(Clone)]
pub struct ReadGrantChange {
    pub transition: ReadGrantTransition,
    pub conversation_id: ConversationId,
    /// Absent only for a revoke of a device whose binding is gone: the grant
    /// is then found by its credential.
    pub receiver_id: Option<String>,
    pub credential_id: CredentialId,
    pub initiator: ConversationCaller,
    pub at_ms: u64,
}

/// The gateway's table of read grants. Grants and follow rules are separate
/// tables: this is the owner's side only, and a follower's choice of what to
/// keep never reaches it.
pub trait ReadGrants: Send + Sync {
    /// Whether `receiver_id` holds a grant on `id`.
    fn is_granted<'a>(
        &'a self,
        id: &'a ConversationId,
        receiver_id: &'a str,
    ) -> ConversationFuture<'a, bool>;
    /// Apply one change. `false` when it changed nothing (granting a grant
    /// already held, revoking one not held). Every admitted request is
    /// journaled under its request id, a no-op too (before equal to after),
    /// and a retry of the same request answers from that row and changes
    /// nothing (rows G9, G16). A retry is admitted like a new request first:
    /// ownership for both, and not deleted for a grant.
    fn change(&self, change: ReadGrantChange) -> ConversationFuture<'_, bool>;
    /// The grants on one conversation, oldest first.
    fn grants<'a>(&'a self, id: &'a ConversationId) -> ConversationFuture<'a, Vec<ReadGrant>>;
}

/// The owner's share commands: `conversation.share`, `.unshare`, `.shares`.
pub struct ShareConversation<'a> {
    pub conversations: &'a dyn ConversationRepository,
    pub receivers: &'a dyn ReceiverAuthority,
    pub grants: &'a dyn ReadGrants,
    /// The credential registry, read for the principal a share target names.
    pub access: &'a dyn AccessReader,
}

impl ShareConversation<'_> {
    /// Grant `credential`'s receiver Read on `id`. The credential must be an
    /// active paired device or peer gateway of the caller, or
    /// `ShareTargetNotPaired`; the store refuses a conversation that is not
    /// the caller's or is deleted, in the transaction that writes the grant
    /// (row G8). Either kind of party reads only what it is granted
    /// (`docs/design/auth/peer-gateways.md`, row H8).
    pub async fn share(
        &self,
        caller: ConversationCaller,
        id: ConversationId,
        credential: CredentialId,
        at_ms: u64,
    ) -> Result<bool, ConversationError> {
        caller.actor()?;
        let binding = self
            .receivers
            .resolve(&credential)
            .await
            .map_err(|_| ConversationError::Metadata)?
            .filter(|binding| {
                binding.active
                    && binding.credential_id == credential
                    && binding.organization_id == caller.organization_id
                    && binding.owner_id == caller.principal_id
            })
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
        self.grants
            .change(ReadGrantChange {
                transition: ReadGrantTransition::Grant,
                conversation_id: id,
                receiver_id: Some(binding.receiver_id),
                credential_id: credential,
                initiator: caller,
                at_ms,
            })
            .await
    }

    /// Revoke `credential`'s grant on `id`. Allowed on a deleted conversation
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
            .change(ReadGrantChange {
                transition: ReadGrantTransition::Revoke,
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
    ) -> Result<Vec<ReadGrant>, ConversationError> {
        self.owner_of(caller, id).await?;
        self.grants.grants(id).await
    }

    /// The caller owns `id`, deleted or not.
    async fn owner_of(
        &self,
        caller: &ConversationCaller,
        id: &ConversationId,
    ) -> Result<(), ConversationError> {
        caller.actor()?;
        let conversation = load_conversation(self.conversations, id)
            .await?
            .ok_or(ConversationError::NotFound)?;
        if !conversation.allows(&caller.organization_id, &caller.principal_id) {
            return Err(ConversationError::NotFound);
        }
        Ok(())
    }
}
