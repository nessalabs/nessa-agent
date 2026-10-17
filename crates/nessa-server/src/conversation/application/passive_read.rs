//! Admission for bounded passive reads, before a record or catalogue source is touched.

use super::metadata_target::load_conversation;
use super::{
    AccessAction, AccessAsk, AccessDecision, AccessDenial, AccessResource, AccessSubject,
    AdmitOwnerPhoneReader, ConversationAccessPolicyEvaluator, ConversationRepository,
    OwnerPhonePolicyEvaluator, OwnerPhoneReceiverAuthority, Reader, RequestShape, ShareGrants,
};
use crate::conversation::domain::ReceiverBinding;
use nessa_auth::{
    application::{
        authorization::AuthorizeAction,
        pairing::OwnerPhonePurposeSource,
        ports::{AccessError, Decision},
        session::AuthenticatedSession,
    },
    domain::{Action, CredentialId, Resource},
};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::conversation::read_scope::{
    CatalogueReadScope, ReadRefusal, ReceiverReadScope,
};
use std::{future::Future, pin::Pin, sync::Arc};

/// Server-owned authority accompanies a published scope without changing its wire ABI.
#[derive(Clone)]
pub struct AdmittedCatalogueRead {
    pub scope: CatalogueReadScope,
    pub reader: Reader,
}
impl AdmittedCatalogueRead {
    /// Relate the captured authority to every public scope selector before I/O.
    pub fn validate_reader(&self) -> Result<(), ReadRefusal> {
        let valid = match &self.reader {
            Reader::OwnerPhone(phone) => {
                let receiver = phone.receiver();
                receiver.receiver_id == self.scope.receiver_id
                    && receiver.organization_id == self.scope.organization_id
                    && receiver.owner_id == self.scope.owner_id
                    && receiver.access_epoch == self.scope.access_epoch
            }
            Reader::PairedDevice { receiver_id } => receiver_id == &self.scope.receiver_id,
            Reader::Owner => false,
        };
        if valid {
            Ok(())
        } else {
            Err(ReadRefusal::Unverifiable)
        }
    }
}

/// The passive read being admitted. Each variant is one read, and asks for
/// that read's published grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassiveRead {
    RecordHead,
    RecordPage,
    CatalogueHead,
    CatalogueManifest,
    CatalogueResolve,
}

/// The grant published for one passive read.
///
/// Admission does not name the grant. The owner of the mapping answers here;
/// a missing grant cannot be admitted.
pub trait PassiveReadGrants: Send + Sync {
    fn grant(&self, read: PassiveRead) -> Option<&'static str>;
}

/// A binding authority reads committed state. A missing or unavailable binding
/// never becomes a caller-chosen receiver identity.
pub trait ReceiverAuthority: Send + Sync {
    fn resolve<'a>(
        &'a self,
        credential_id: &'a CredentialId,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ReceiverBinding>, ReadRefusal>> + Send + 'a>>;
}

/// Admission orders fresh authentication, binding and ownership before source I/O.
pub struct AdmitPassiveRead<'a> {
    pub authorization: AuthorizeAction<'a>,
    pub gateway: &'a Resource,
    pub receivers: &'a dyn ReceiverAuthority,
    pub conversations: &'a dyn ConversationRepository,
    /// Published grant for the read actually being admitted.
    pub grants: &'a dyn PassiveReadGrants,
    /// Which conversations each paired device was granted (`share_grants`).
    pub share_grants: &'a dyn ShareGrants,
    /// Decides, from the binding, the conversation and the grant held,
    /// whether the paired party may read it.
    pub access_policy: Arc<dyn ConversationAccessPolicyEvaluator>,
    pub owner_phone_purposes: &'a dyn OwnerPhonePurposeSource,
    pub owner_phone_receivers: Option<&'a dyn OwnerPhoneReceiverAuthority>,
    pub owner_phone_policy: Arc<dyn OwnerPhonePolicyEvaluator>,
}

impl AdmitPassiveRead<'_> {
    pub async fn catalogue(
        &self,
        session: &AuthenticatedSession,
        receiver_id: &str,
        access_epoch: u64,
        read: PassiveRead,
    ) -> Result<CatalogueReadScope, ReadRefusal> {
        self.catalogue_admission(session, receiver_id, access_epoch, read)
            .await
            .map(|admitted| admitted.scope)
    }

    pub async fn catalogue_admission(
        &self,
        session: &AuthenticatedSession,
        receiver_id: &str,
        access_epoch: u64,
        read: PassiveRead,
    ) -> Result<AdmittedCatalogueRead, ReadRefusal> {
        let (binding, reader) = self
            .binding(session, receiver_id, access_epoch, read)
            .await?;
        if let Reader::OwnerPhone(phone) = &reader {
            phone.admit_catalogue(
                &binding.owner_id,
                &binding.organization_id,
                AccessAction::List,
            )?;
        }
        Ok(AdmittedCatalogueRead {
            scope: CatalogueReadScope {
                receiver_id: binding.receiver_id,
                organization_id: binding.organization_id,
                owner_id: binding.owner_id,
                access_epoch: binding.access_epoch,
            },
            reader,
        })
    }

    /// Whether `session`, admitted to `scope`, may watch its owner's
    /// catalogue. Producer notices follow the receiver's scoped head;
    /// the policy still decides from whom the session signed in as
    /// (`catalogue.watch`). Refused, it is `Forbidden`.
    pub fn catalogue_watch(
        &self,
        session: &AuthenticatedSession,
        scope: &CatalogueReadScope,
    ) -> Result<(), ReadRefusal> {
        let ask = AccessAsk {
            subject: AccessSubject::PairedReader {
                organization: &scope.organization_id,
                signed_in_as: session.context().principal_id(),
                binding_owner: &scope.owner_id,
                receiver: &scope.receiver_id,
            },
            action: AccessAction::CatalogueWatch,
            resource: AccessResource::Catalogue {
                organization: &scope.organization_id,
                owner: &scope.owner_id,
            },
            held: None,
            request: RequestShape::default(),
            target: None,
        };
        paired_refusal(self.access_policy.evaluate(&ask), ReadRefusal::Forbidden)
    }

    pub async fn catalogue_with<T, E, F, Fut>(
        &self,
        session: &AuthenticatedSession,
        receiver_id: &str,
        access_epoch: u64,
        read: PassiveRead,
        source: F,
    ) -> Result<T, ReadRefusal>
    where
        F: FnOnce(CatalogueReadScope) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let scope = self
            .catalogue(session, receiver_id, access_epoch, read)
            .await?;
        source(scope).await.map_err(|_| ReadRefusal::Unverifiable)
    }

    /// Invoke a source only after exact admission. A source error is an
    /// unverifiable read, never an authorization decision.
    pub async fn read_with<T, E, F, Fut>(
        &self,
        session: &AuthenticatedSession,
        conversation_id: &ConversationId,
        receiver_id: &str,
        access_epoch: u64,
        read: PassiveRead,
        source: F,
    ) -> Result<T, ReadRefusal>
    where
        F: FnOnce(ReceiverReadScope) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let scope = self
            .execute(session, conversation_id, receiver_id, access_epoch, read)
            .await?;
        source(scope).await.map_err(|_| ReadRefusal::Unverifiable)
    }

    pub async fn execute(
        &self,
        session: &AuthenticatedSession,
        conversation_id: &ConversationId,
        receiver_id: &str,
        access_epoch: u64,
        read: PassiveRead,
    ) -> Result<ReceiverReadScope, ReadRefusal> {
        let (binding, reader) = self
            .binding(session, receiver_id, access_epoch, read)
            .await?;
        let conversation = load_conversation(self.conversations, conversation_id)
            .await
            .map_err(|_| ReadRefusal::Unverifiable)?
            .ok_or(ReadRefusal::WrongOwner)?;
        if let Reader::OwnerPhone(phone) = &reader {
            phone.admit_conversation(&conversation, AccessAction::Read)?;
        } else {
            // Only pairing makes a receiver binding, so this reader is a paired
            // party, and the policy reads it with the grant it holds there, of
            // any role, on its grantor's conversation, not deleted (rows G1, G3,
            // G4). Refused, it is told what a missing conversation is told.
            let held = self
                .share_grants
                .held_capabilities(conversation_id, &binding.receiver_id)
                .await
                .map_err(|_| ReadRefusal::Unverifiable)?;
            let ask = AccessAsk {
                subject: AccessSubject::PairedReader {
                    organization: &binding.organization_id,
                    signed_in_as: session.context().principal_id(),
                    binding_owner: &binding.owner_id,
                    receiver: &binding.receiver_id,
                },
                action: AccessAction::Read,
                resource: AccessResource::of(&conversation),
                held,
                request: RequestShape::default(),
                target: None,
            };
            paired_refusal(self.access_policy.evaluate(&ask), ReadRefusal::WrongOwner)?;
        }
        Ok(ReceiverReadScope {
            receiver_id: binding.receiver_id,
            organization_id: binding.organization_id,
            owner_id: binding.owner_id,
            conversation_id: conversation_id.clone(),
            access_epoch: binding.access_epoch,
        })
    }

    async fn binding(
        &self,
        session: &AuthenticatedSession,
        receiver_id: &str,
        access_epoch: u64,
        read: PassiveRead,
    ) -> Result<(ReceiverBinding, Reader), ReadRefusal> {
        if receiver_id.is_empty() || receiver_id.len() > 128 || access_epoch == 0 {
            return Err(ReadRefusal::InvalidRequest);
        }
        // Admission asks only for the grant this port returns for the read.
        // A read with no published grant, or a grant that is not an action,
        // cannot be admitted.
        let action_name = self.grants.grant(read).ok_or(ReadRefusal::Unverifiable)?;
        let action = Action::new(action_name).map_err(|_| ReadRefusal::Unverifiable)?;
        match self
            .authorization
            .execute(session, &action, self.gateway)
            .await
        {
            Ok(Decision::Allow) => {}
            Ok(Decision::Deny) => return Err(ReadRefusal::Forbidden),
            Err(error) => return Err(access_refusal(error)),
        }
        let phone = AdmitOwnerPhoneReader {
            purposes: self.owner_phone_purposes,
            holding: self.owner_phone_receivers,
            clock: self.authorization.clock,
            gateway: self.gateway,
            policy: self.owner_phone_policy.clone(),
            access_policy: Some(self.access_policy.clone()),
        }
        .execute(session)
        .await?;
        let (binding, reader) = if let Some(phone) = phone {
            (
                phone.receiver().clone(),
                Reader::OwnerPhone(Box::new(phone)),
            )
        } else {
            let binding = self
                .receivers
                .resolve(session.context().credential_id())
                .await?
                .ok_or(ReadRefusal::Unverifiable)?;
            let reader = Reader::of(Some(&binding));
            (binding, reader)
        };
        if !binding.active {
            return Err(ReadRefusal::Unauthorized);
        }
        // The binding is the session's own: the credential it was resolved by
        // and the organization it reads in. Its owner is the grantor whose
        // conversations it reads, not the reader, so it is not compared with
        // the session's principal: a paired device signs in as its owner, a
        // peer gateway as itself (`docs/design/auth/peer-gateways.md`), and
        // both read only what was granted to the binding's receiver.
        //
        // The grantor's liveness is not checked here. The grantor is the
        // pairing initiator: an active admin holding `credential.manage` when
        // it paired, and no runtime operation disables a membership or takes
        // that role away. A device stops with its owner because it signs in
        // as them; a peer would not. A change that lets a membership be
        // disabled or demoted at runtime, or gives another admin a way to
        // pair, must add the pairing initiator's membership to this admission
        // in the same change.
        if binding.credential_id != *session.context().credential_id()
            || binding.organization_id != *session.context().organization_id()
        {
            return Err(ReadRefusal::Unverifiable);
        }
        if binding.receiver_id != receiver_id {
            return Err(ReadRefusal::WrongReceiver);
        }
        if binding.access_epoch != access_epoch {
            return Err(ReadRefusal::StaleEpoch);
        }
        Ok((binding, reader))
    }
}

/// A paired party's access decision as a read refusal: `refused` when the
/// policy refused, `Unverifiable` when it could not decide.
fn paired_refusal(decision: AccessDecision, refused: ReadRefusal) -> Result<(), ReadRefusal> {
    match decision {
        AccessDecision::Allow { .. } => Ok(()),
        AccessDecision::Deny(AccessDenial::Unavailable) => Err(ReadRefusal::Unverifiable),
        AccessDecision::Deny(denial) => {
            tracing::debug!(?denial, "a paired party's conversation access was refused");
            Err(refused)
        }
    }
}

/// The refusal a passive read answers with when access was not granted:
/// the gateway's policy for which access errors a reader may act on.
pub(crate) fn access_refusal(error: AccessError) -> ReadRefusal {
    match error {
        AccessError::Denied => ReadRefusal::Forbidden,
        AccessError::Unavailable | AccessError::StaleRevision | AccessError::Unsupported => {
            ReadRefusal::Unverifiable
        }
        AccessError::InvalidCredential
        | AccessError::CredentialRevoked
        | AccessError::CredentialExpired
        | AccessError::InactiveMembership
        | AccessError::IdentityMismatch => ReadRefusal::Unauthorized,
    }
}
