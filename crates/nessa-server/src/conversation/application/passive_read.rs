//! Admission for bounded passive reads, before a record or catalogue source is touched.

use super::ConversationRepository;
use crate::conversation::domain::{ConversationId, ReceiverBinding};
use nessa_auth::{
    application::{
        authorization::AuthorizeAction,
        ports::{AccessError, Decision},
        session::AuthenticatedSession,
    },
    domain::{Action, CredentialId, OrganizationId, PrincipalId, Resource},
};
use nessa_sync::replication::domain::{Id, Scope};
use std::{future::Future, pin::Pin};

/// A binding authority reads committed state. A missing or unavailable binding
/// never becomes a caller-chosen receiver identity.
pub trait ReceiverAuthority: Send + Sync {
    fn resolve<'a>(
        &'a self,
        credential_id: &'a CredentialId,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ReceiverBinding>, ReadRefusal>> + Send + 'a>>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadRefusal {
    InvalidRequest,
    Unauthorized,
    Forbidden,
    WrongOwner,
    WrongReceiver,
    StaleEpoch,
    Unverifiable,
}

impl From<AccessError> for ReadRefusal {
    fn from(error: AccessError) -> Self {
        match error {
            AccessError::Unavailable | AccessError::StaleRevision | AccessError::Unsupported => {
                Self::Unverifiable
            }
            AccessError::InvalidCredential
            | AccessError::CredentialRevoked
            | AccessError::CredentialExpired
            | AccessError::InactiveMembership
            | AccessError::IdentityMismatch => Self::Unauthorized,
        }
    }
}

/// Exact receiver and ownership portion of a source scope. The physical source
/// contributes its own origin, stream, incarnation and schema after admission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiverReadScope {
    pub receiver_id: String,
    pub organization_id: OrganizationId,
    pub owner_id: PrincipalId,
    pub conversation_id: ConversationId,
    pub access_epoch: u64,
}

/// Owner-scoped catalogue selector, independent of any conversation ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogueReadScope {
    pub receiver_id: String,
    pub organization_id: OrganizationId,
    pub owner_id: PrincipalId,
    pub access_epoch: u64,
}

/// Admission orders fresh authentication, binding and ownership before source I/O.
pub struct AdmitPassiveRead<'a> {
    pub authorization: AuthorizeAction<'a>,
    pub gateway: &'a Resource,
    pub receivers: &'a dyn ReceiverAuthority,
    pub conversations: &'a dyn ConversationRepository,
}

impl AdmitPassiveRead<'_> {
    pub async fn catalogue(
        &self,
        session: &AuthenticatedSession,
        receiver_id: &str,
        access_epoch: u64,
    ) -> Result<CatalogueReadScope, ReadRefusal> {
        let binding = self.binding(session, receiver_id, access_epoch).await?;
        Ok(CatalogueReadScope {
            receiver_id: binding.receiver_id,
            organization_id: binding.organization_id,
            owner_id: binding.owner_id,
            access_epoch: binding.access_epoch,
        })
    }

    pub async fn catalogue_with<T, E, F, Fut>(
        &self,
        session: &AuthenticatedSession,
        receiver_id: &str,
        access_epoch: u64,
        source: F,
    ) -> Result<T, ReadRefusal>
    where
        F: FnOnce(CatalogueReadScope) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let scope = self.catalogue(session, receiver_id, access_epoch).await?;
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
        source: F,
    ) -> Result<T, ReadRefusal>
    where
        F: FnOnce(ReceiverReadScope) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let scope = self
            .execute(session, conversation_id, receiver_id, access_epoch)
            .await?;
        source(scope).await.map_err(|_| ReadRefusal::Unverifiable)
    }

    pub async fn execute(
        &self,
        session: &AuthenticatedSession,
        conversation_id: &ConversationId,
        receiver_id: &str,
        access_epoch: u64,
    ) -> Result<ReceiverReadScope, ReadRefusal> {
        let binding = self.binding(session, receiver_id, access_epoch).await?;
        let conversation = self
            .conversations
            .load(conversation_id)
            .await
            .map_err(|_| ReadRefusal::Unverifiable)?
            .ok_or(ReadRefusal::WrongOwner)?;
        if conversation
            .check_access(&binding.organization_id, &binding.owner_id)
            .is_err()
        {
            return Err(ReadRefusal::WrongOwner);
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
    ) -> Result<ReceiverBinding, ReadRefusal> {
        if receiver_id.is_empty() || receiver_id.len() > 128 || access_epoch == 0 {
            return Err(ReadRefusal::InvalidRequest);
        }
        let action = Action::new("conversation.read").expect("static action");
        match self
            .authorization
            .execute(session, &action, self.gateway)
            .await
        {
            Ok(Decision::Allow) => {}
            Ok(Decision::Deny) => return Err(ReadRefusal::Forbidden),
            Err(error) => return Err(error.into()),
        }
        let binding = self
            .receivers
            .resolve(session.context().credential_id())
            .await?
            .ok_or(ReadRefusal::Unverifiable)?;
        if !binding.active {
            return Err(ReadRefusal::Unauthorized);
        }
        if binding.credential_id != *session.context().credential_id()
            || binding.organization_id != *session.context().organization_id()
        {
            return Err(ReadRefusal::Unverifiable);
        }
        if binding.owner_id != *session.context().principal_id() {
            return Err(ReadRefusal::WrongOwner);
        }
        if binding.receiver_id != receiver_id {
            return Err(ReadRefusal::WrongReceiver);
        }
        if binding.access_epoch != access_epoch {
            return Err(ReadRefusal::StaleEpoch);
        }
        Ok(binding)
    }
}

/// Encode the trusted passive receiver and numeric epoch into opaque sync IDs.
pub(crate) fn passive_read_selector(receiver: &str, epoch: u64) -> Result<(Id, Id), ReadRefusal> {
    let receiver = Id::new(receiver).map_err(|_| ReadRefusal::Unverifiable)?;
    let epoch = Id::new(format!("epoch-{epoch}")).map_err(|_| ReadRefusal::Unverifiable)?;
    Ok((receiver, epoch))
}

pub(crate) fn validate_passive_read_selector(
    receiver: &str,
    epoch: u64,
    scope: &Scope,
) -> Result<(), ReadRefusal> {
    let (receiver, epoch) = passive_read_selector(receiver, epoch)?;
    if scope.receiver() != &receiver {
        return Err(ReadRefusal::WrongReceiver);
    }
    if scope.access_epoch() != &epoch {
        return Err(ReadRefusal::StaleEpoch);
    }
    Ok(())
}
