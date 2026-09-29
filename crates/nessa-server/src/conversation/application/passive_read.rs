//! Admission for bounded passive reads, before a record or catalogue source is touched.

use super::ConversationRepository;
use crate::conversation::domain::ConversationId;
use nessa_auth::{
    application::{
        authorization::AuthorizeAction,
        ports::{AccessError, Decision},
        session::AuthenticatedSession,
    },
    domain::{Action, CredentialId, OrganizationId, PrincipalId, Resource},
};
use std::{future::Future, pin::Pin};

/// Durable server-owned binding of one credential to a paired receiver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiverBinding {
    pub receiver_id: String,
    pub credential_id: CredentialId,
    pub organization_id: OrganizationId,
    pub owner_id: PrincipalId,
    pub access_epoch: u64,
    pub active: bool,
}

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

    pub async fn catalogue_with<T, F, Fut>(
        &self,
        session: &AuthenticatedSession,
        receiver_id: &str,
        access_epoch: u64,
        source: F,
    ) -> Result<T, ReadRefusal>
    where
        F: FnOnce(CatalogueReadScope) -> Fut,
        Fut: Future<Output = Result<T, ReadRefusal>>,
    {
        let scope = self.catalogue(session, receiver_id, access_epoch).await?;
        source(scope).await
    }

    /// Invoke a source only after exact admission. A source error is an
    /// unverifiable read, never an authorization decision.
    pub async fn read_with<T, F, Fut>(
        &self,
        session: &AuthenticatedSession,
        conversation_id: &ConversationId,
        receiver_id: &str,
        access_epoch: u64,
        source: F,
    ) -> Result<T, ReadRefusal>
    where
        F: FnOnce(ReceiverReadScope) -> Fut,
        Fut: Future<Output = Result<T, ReadRefusal>>,
    {
        let scope = self
            .execute(session, conversation_id, receiver_id, access_epoch)
            .await?;
        source(scope).await
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
            Err(AccessError::Unavailable | AccessError::Unsupported) => {
                return Err(ReadRefusal::Unverifiable)
            }
            Err(_) => return Err(ReadRefusal::Unauthorized),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        agents::domain::AgentId,
        conversation::{
            domain::{Conversation, ConversationApprovalMode, ConversationModelId},
            infrastructure::LocalConversationStore,
        },
    };
    use nessa_auth::{
        adapters::cedar::CedarPolicyEvaluator,
        application::{
            ports::{
                AccessReader, AccessSnapshot, Clock, CredentialEvidence, CredentialVerifier,
                PortFuture, VerifiedCredential,
            },
            session::AuthenticateSession,
        },
        domain::{
            AudienceId, Credential, Grant, Membership, MembershipId, MembershipRole,
            MembershipStatus, ResourceId,
        },
    };
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };

    struct Access(Mutex<AccessSnapshot>);
    impl AccessReader for Access {
        fn read<'a>(&'a self, _: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
            Box::pin(async { Ok(self.0.lock().unwrap().clone()) })
        }
    }
    impl CredentialVerifier for Access {
        fn verify<'a>(
            &'a self,
            _: &'a CredentialEvidence,
            _: &'a AudienceId,
        ) -> PortFuture<'a, VerifiedCredential> {
            Box::pin(async {
                Ok(VerifiedCredential {
                    credential_id: CredentialId::new("credential").unwrap(),
                    expires_at: None,
                })
            })
        }
    }
    struct FixedClock;
    impl Clock for FixedClock {
        fn unix_milliseconds(&self) -> u64 {
            150_000
        }
    }
    struct Bindings(Mutex<Result<Option<ReceiverBinding>, ReadRefusal>>);
    impl ReceiverAuthority for Bindings {
        fn resolve<'a>(
            &'a self,
            _: &'a CredentialId,
        ) -> Pin<Box<dyn Future<Output = Result<Option<ReceiverBinding>, ReadRefusal>> + Send + 'a>>
        {
            Box::pin(async { self.0.lock().unwrap().clone() })
        }
    }
    fn access() -> Access {
        let org = OrganizationId::new("org").unwrap();
        let owner = PrincipalId::new("owner").unwrap();
        let resource = Resource::new(org.clone(), ResourceId::new("gateway").unwrap());
        Access(Mutex::new(AccessSnapshot {
            credential: Credential::new(
                CredentialId::new("credential").unwrap(),
                owner.clone(),
                org.clone(),
                AudienceId::new("gateway").unwrap(),
                100,
                200,
                vec![Grant::new(
                    Action::new("conversation.read").unwrap(),
                    resource,
                )],
            )
            .unwrap(),
            membership: Membership::new(
                MembershipId::new("member").unwrap(),
                owner,
                org,
                MembershipRole::Member,
                MembershipStatus::Active,
            ),
            revision: 1,
        }))
    }
    fn binding() -> ReceiverBinding {
        ReceiverBinding {
            receiver_id: "receiver".into(),
            credential_id: CredentialId::new("credential").unwrap(),
            organization_id: OrganizationId::new("org").unwrap(),
            owner_id: PrincipalId::new("owner").unwrap(),
            access_epoch: 7,
            active: true,
        }
    }

    #[tokio::test]
    async fn every_refusal_precedes_source_and_valid_owner_reaches_it() {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("conversations");
        nessa_local_storage::create_directory(&private).unwrap();
        let path = private.join("metadata.sqlite3");
        let conversations = LocalConversationStore::open(&path).unwrap();
        let id = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
        conversations
            .create(
                Conversation::new(
                    id.clone(),
                    OrganizationId::new("org").unwrap(),
                    PrincipalId::new("owner").unwrap(),
                    "panel".into(),
                    "create".into(),
                    1,
                    AgentId::Claude,
                    ConversationModelId::new("model").unwrap(),
                    ConversationApprovalMode::Ask,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let access = access();
        let clock = FixedClock;
        let policy = CedarPolicyEvaluator::new().unwrap();
        let gateway = Resource::new(
            OrganizationId::new("org").unwrap(),
            ResourceId::new("gateway").unwrap(),
        );
        let bindings = Bindings(Mutex::new(Ok(Some(binding()))));
        let session = AuthenticateSession {
            verifier: &access,
            access: &access,
            clock: &clock,
        }
        .execute(
            &CredentialEvidence::new(b"secret".to_vec()).unwrap(),
            &AudienceId::new("gateway").unwrap(),
        )
        .await
        .unwrap();
        let admit = AdmitPassiveRead {
            authorization: AuthorizeAction {
                access: &access,
                clock: &clock,
                policy: &policy,
            },
            gateway: &gateway,
            receivers: &bindings,
            conversations: &conversations,
        };
        let calls = AtomicUsize::new(0);
        let source = |_: ReceiverReadScope| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Ok(()) }
        };
        assert_eq!(
            admit.read_with(&session, &id, "receiver", 7, source).await,
            Ok(())
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            admit
                .read_with(&session, &id, "receiver", 7, |_| {
                    *bindings.0.lock().unwrap() = Ok(Some(ReceiverBinding {
                        active: false,
                        ..binding()
                    }));
                    async { Ok(()) }
                })
                .await,
            Ok(())
        );
        assert_eq!(
            admit
                .read_with(&session, &id, "receiver", 7, |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Ok(()) }
                })
                .await,
            Err(ReadRefusal::Unauthorized)
        );
        *bindings.0.lock().unwrap() = Ok(Some(binding()));

        for (receiver, epoch, expected) in [
            ("wrong", 7, ReadRefusal::WrongReceiver),
            ("receiver", 6, ReadRefusal::StaleEpoch),
            ("", 7, ReadRefusal::InvalidRequest),
        ] {
            assert_eq!(
                admit
                    .read_with(&session, &id, receiver, epoch, |_| {
                        calls.fetch_add(1, Ordering::SeqCst);
                        async { Ok(()) }
                    })
                    .await,
                Err(expected)
            );
        }
        let other = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
        conversations
            .create(
                Conversation::new(
                    other.clone(),
                    OrganizationId::new("org").unwrap(),
                    PrincipalId::new("another-owner").unwrap(),
                    "panel".into(),
                    "create".into(),
                    1,
                    AgentId::Claude,
                    ConversationModelId::new("model").unwrap(),
                    ConversationApprovalMode::Ask,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            admit
                .read_with(&session, &other, "receiver", 7, |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Ok(()) }
                })
                .await,
            Err(ReadRefusal::WrongOwner)
        );
        let catalogue = admit.catalogue(&session, "receiver", 7).await.unwrap();
        assert_eq!(catalogue.owner_id, PrincipalId::new("owner").unwrap());
        assert_eq!(
            catalogue.organization_id,
            OrganizationId::new("org").unwrap()
        );
        assert_eq!(
            admit
                .catalogue_with(&session, "wrong", 7, |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Ok(()) }
                })
                .await,
            Err(ReadRefusal::WrongReceiver)
        );
        *bindings.0.lock().unwrap() = Ok(Some(ReceiverBinding {
            owner_id: PrincipalId::new("other-owner").unwrap(),
            ..binding()
        }));
        assert_eq!(
            admit
                .read_with(&session, &id, "receiver", 7, |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Ok(()) }
                })
                .await,
            Err(ReadRefusal::WrongOwner)
        );
        assert_eq!(
            admit
                .catalogue_with(&session, "receiver", 7, |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Ok(()) }
                })
                .await,
            Err(ReadRefusal::WrongOwner)
        );
        *bindings.0.lock().unwrap() = Err(ReadRefusal::Unverifiable);
        assert_eq!(
            admit
                .read_with(&session, &id, "receiver", 7, |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Ok(()) }
                })
                .await,
            Err(ReadRefusal::Unverifiable)
        );
        *bindings.0.lock().unwrap() = Ok(Some(ReceiverBinding {
            active: false,
            ..binding()
        }));
        assert_eq!(
            admit
                .read_with(&session, &id, "receiver", 7, |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Ok(()) }
                })
                .await,
            Err(ReadRefusal::Unauthorized)
        );
        *bindings.0.lock().unwrap() = Ok(Some(binding()));
        assert_eq!(
            admit
                .read_with(&session, &id, "receiver", 7, |_| {
                    access
                        .0
                        .lock()
                        .unwrap()
                        .credential
                        .revoke(151, nessa_auth::domain::Initiator::LocalOperator)
                        .unwrap();
                    async { Ok(()) }
                })
                .await,
            Ok(())
        );
        assert_eq!(
            admit
                .read_with(&session, &id, "receiver", 7, |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Ok(()) }
                })
                .await,
            Err(ReadRefusal::Unauthorized)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
