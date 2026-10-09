use super::super::passive_read::PUBLISHED_PASSIVE_READ_GRANTS;
use crate::conversation::application::{
    access_refusal, AdmitPassiveRead, CatalogueChangeWatch, CatalogueWatchError,
    CatalogueWatchState, ConversationError, PassiveRead,
};
use crate::product::socket::close_reason;
use crate::product::state::ProductRouteState;
use nessa_auth::application::ports::{AccessError, Decision};
use nessa_auth::application::{authorization::AuthorizeAction, session::AuthenticatedSession};
use nessa_auth::domain::{Action, OrganizationId, PrincipalId};
use nessa_protocol::conversation::{domain::ConversationId, read_scope::ReadRefusal};
use nessa_protocol::product::generated::{
    product_method, ConversationWatchCatalogueParams, ConversationWatchRecordsParams,
    MAX_CHANGE_WATCH_ID_BYTES, MAX_CONNECTION_CATALOGUE_WATCHES, MAX_CONNECTION_RECORD_WATCHES,
};
use nessa_protocol::product::passive_read::decode_epoch;
use nessa_protocol::product_contract::generated::{
    ChangeWatchEndReason, ChangeWatchErrorCode, SessionCloseReason,
};
use nessa_sdk::application::agent_execution::sessions::{
    ChangeWatchError, ChangeWatchState, CommittedChangeWatch,
};
use nessa_sync::replication::domain::Id;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use uuid::Uuid;

/// One current connection's monotonic namespace, never a source or access identity.
pub(in crate::product) struct WatchToken {
    namespace: Uuid,
    accepted: u64,
}

impl WatchToken {
    pub fn new(namespace: Uuid) -> Self {
        Self {
            namespace,
            accepted: 0,
        }
    }

    pub fn can_accept(&self, count: usize) -> bool {
        u64::try_from(count)
            .ok()
            .and_then(|count| self.accepted.checked_add(count))
            .is_some()
    }

    pub fn next(&self) -> Result<String, ChangeWatchErrorCode> {
        let next = self
            .accepted
            .checked_add(1)
            .ok_or(ChangeWatchErrorCode::WatchCapacity)?;
        Ok(format!("{}-{next}", self.namespace))
    }

    // Advance only after actual producer installation. No failed-registration gaps.
    pub fn accepted(&mut self) {
        self.accepted += 1;
    }

    pub fn owns(&self, id: &str) -> bool {
        if id.len() > MAX_CHANGE_WATCH_ID_BYTES {
            return false;
        }
        let Some((namespace, counter)) = id.rsplit_once('-') else {
            return false;
        };
        let Ok(counter) = counter.parse::<u64>() else {
            return false;
        };
        counter > 0
            && counter <= self.accepted
            && id == format!("{}-{counter}", self.namespace)
            && namespace == self.namespace.to_string()
    }
}

/// A paired receiver's binding. Both fields are present, or the watch is an
/// owner-session watch and this is absent.
#[derive(Clone, PartialEq, Eq)]
pub(in crate::product) struct PairedWatch {
    receiver: String,
    epoch: u64,
}

#[derive(Clone, PartialEq, Eq)]
pub(in crate::product) enum WatchSelector {
    Records {
        conversation: ConversationId,
        paired: Option<PairedWatch>,
    },
    Catalogue {
        paired: Option<PairedWatch>,
    },
}

/// Both receiver fields, or neither. Exactly one is not a watch.
fn paired_watch(
    receiver_id: Option<String>,
    access_epoch: Option<String>,
) -> Result<Option<PairedWatch>, ChangeWatchErrorCode> {
    let invalid = ChangeWatchErrorCode::InvalidRequest;
    match (receiver_id, access_epoch) {
        (None, None) => Ok(None),
        (Some(receiver), Some(epoch)) => Ok(Some(PairedWatch {
            receiver: Id::new(receiver).map_err(|_| invalid)?.as_str().to_owned(),
            epoch: decode_epoch(&epoch).map_err(|_| invalid)?,
        })),
        _ => Err(invalid),
    }
}

impl WatchSelector {
    pub fn decode(method: &str, params: Value) -> Result<Self, ChangeWatchErrorCode> {
        let invalid = || ChangeWatchErrorCode::InvalidRequest;
        if method == product_method::CONVERSATION_WATCH_RECORDS {
            let value: ConversationWatchRecordsParams =
                serde_json::from_value(params).map_err(|_| invalid())?;
            Ok(Self::Records {
                conversation: ConversationId::new(&value.conversation_id).map_err(|_| invalid())?,
                paired: paired_watch(value.receiver_id, value.access_epoch)?,
            })
        } else if method == product_method::CONVERSATION_WATCH_CATALOGUE {
            let value: ConversationWatchCatalogueParams =
                serde_json::from_value(params).map_err(|_| invalid())?;
            Ok(Self::Catalogue {
                paired: paired_watch(value.receiver_id, value.access_epoch)?,
            })
        } else {
            Err(invalid())
        }
    }

    /// How many targets of this kind one connection may hold, as published by
    /// the product schema.
    pub fn connection_limit(&self) -> usize {
        match self {
            Self::Records { .. } => MAX_CONNECTION_RECORD_WATCHES,
            Self::Catalogue { .. } => MAX_CONNECTION_CATALOGUE_WATCHES,
        }
    }

    pub fn same_kind(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::Records { .. }, Self::Records { .. })
                | (Self::Catalogue { .. }, Self::Catalogue { .. })
        )
    }

    /// Current authority for this selector: credential, policy, and either a
    /// paired receiver or the session's own `conversation.write` grant plus
    /// ownership. Registration and every notice ask this same function.
    async fn admit(
        &self,
        state: &ProductRouteState,
        session: &AuthenticatedSession,
    ) -> Result<Admitted, WatchRefusal> {
        let current = super::super::socket::watch_identity(state, session)
            .await
            .map_err(WatchRefusal::Access)?;
        let admitted = self.admit_current(state, &current).await?;
        super::super::socket::watch_browser_present(state, &current)
            .await
            .map_err(WatchRefusal::Access)?;
        Ok(admitted)
    }

    /// Selector admission alone, for a session whose identity and browser
    /// presence the caller has just checked. A paired target is admitted as
    /// the head it follows. An owner session uses `conversation.write` and
    /// ownership, and does not require passive read to be composed.
    async fn admit_current(
        &self,
        state: &ProductRouteState,
        session: &AuthenticatedSession,
    ) -> Result<Admitted, WatchRefusal> {
        match self {
            Self::Records {
                paired: Some(paired),
                conversation,
            } => {
                // Watch methods publish no grant of their own. A paired records
                // watch is admitted as a record head: the same read it follows,
                // through that read's manifest grant.
                passive_admission(state)?
                    .execute(
                        session,
                        conversation,
                        &paired.receiver,
                        paired.epoch,
                        PassiveRead::RecordHead,
                    )
                    .await
                    .map_err(WatchRefusal::Read)?;
                Ok(Admitted::Records(conversation.clone()))
            }
            Self::Catalogue {
                paired: Some(paired),
            } => {
                let scope = passive_admission(state)?
                    .catalogue(
                        session,
                        &paired.receiver,
                        paired.epoch,
                        PassiveRead::CatalogueHead,
                    )
                    .await
                    .map_err(WatchRefusal::Read)?;
                Ok(Admitted::Catalogue(CatalogueOwner {
                    organization_id: scope.organization_id,
                    owner_id: scope.owner_id,
                }))
            }
            Self::Records {
                paired: None,
                conversation,
            } => admit_owned_records(state, session, conversation).await,
            Self::Catalogue { paired: None } => admit_owned_catalogue(state, session).await,
        }
    }

    /// The connection's periodic re-check of its live watches (row A3). The
    /// connection's own refresh has just confirmed `current` (identity and
    /// browser presence) and passes it in, so this asks only the selector's
    /// admission, once per distinct target, one after another. Returns one
    /// result per entry of `selectors`, in order.
    pub async fn recheck(
        selectors: &[Self],
        state: &ProductRouteState,
        current: &AuthenticatedSession,
    ) -> Vec<Result<(), WatchRefusal>> {
        let mut results: Vec<Result<(), WatchRefusal>> = Vec::with_capacity(selectors.len());
        for (index, selector) in selectors.iter().enumerate() {
            let earlier = selectors[..index]
                .iter()
                .position(|other| other == selector);
            let result = match earlier {
                Some(earlier) => results[earlier],
                None => selector.admit_current(state, current).await.map(|_| ()),
            };
            results.push(result);
        }
        results
    }

    pub async fn authorize(
        &self,
        state: &ProductRouteState,
        session: &AuthenticatedSession,
    ) -> Result<(), WatchRefusal> {
        self.admit(state, session).await.map(|_| ())
    }

    pub async fn install(
        &self,
        state: &ProductRouteState,
        session: &AuthenticatedSession,
        interest: &AtomicBool,
    ) -> Result<WatchHandle, ChangeWatchErrorCode> {
        // Reuse the actual current snapshot owner without a head/read lease.
        let admitted = self
            .admit(state, session)
            .await
            .map_err(WatchRefusal::code)?;
        if !interest.load(Ordering::Acquire) {
            return Err(ChangeWatchErrorCode::WatchClosed);
        }
        match admitted {
            Admitted::Records(conversation) => state
                .record_watches
                .as_ref()
                .ok_or(ChangeWatchErrorCode::TemporarilyUnavailable)?
                .watch(&conversation)
                .map(WatchHandle::Records)
                .map_err(|error| match error {
                    ChangeWatchError::Capacity => ChangeWatchErrorCode::WatchCapacity,
                    ChangeWatchError::Closed => ChangeWatchErrorCode::WatchClosed,
                }),
            Admitted::Catalogue(admitted) => state
                .catalogue_watches
                .as_ref()
                .ok_or(ChangeWatchErrorCode::TemporarilyUnavailable)?
                .watch(&admitted.organization_id, &admitted.owner_id)
                .map(WatchHandle::Catalogue)
                .map_err(|error| match error {
                    CatalogueWatchError::Capacity => ChangeWatchErrorCode::WatchCapacity,
                }),
        }
    }
}

/// The catalogue watch key. Both admission paths install the same organization
/// and owner; a paired receiver is not part of the key.
struct CatalogueOwner {
    organization_id: OrganizationId,
    owner_id: PrincipalId,
}

/// What current admission established for one selector: the stable record
/// target, or the catalogue owner.
enum Admitted {
    Records(ConversationId),
    Catalogue(CatalogueOwner),
}

fn passive_admission(state: &ProductRouteState) -> Result<AdmitPassiveRead<'_>, WatchRefusal> {
    let (receivers, conversations) = state
        .passive_read
        .as_ref()
        .ok_or(WatchRefusal::Unavailable)?;
    Ok(AdmitPassiveRead {
        authorization: AuthorizeAction {
            access: state.access.as_ref(),
            clock: state.clock.as_ref(),
            policy: state.policy.as_ref(),
        },
        gateway: &state.gateway,
        receivers: receivers.as_ref(),
        conversations: conversations.as_ref(),
        grants: &PUBLISHED_PASSIVE_READ_GRANTS,
    })
}

/// `conversation.write` for this session, the same grant `conversation.read`,
/// `conversation.list`, and `conversation.observe` already require. A deny is
/// forbidden. An access error stays a read refusal so the watch's close
/// mapping is unchanged. A paired watch is admitted separately, as the head
/// it follows, through that head's manifest grant. This owner path does not
/// ask for that grant and does not admit record pages.
async fn authorize_owner(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
) -> Result<(), WatchRefusal> {
    let action = Action::new("conversation.write").expect("static action");
    let authorization = AuthorizeAction {
        access: state.access.as_ref(),
        clock: state.clock.as_ref(),
        policy: state.policy.as_ref(),
    };
    match authorization
        .execute(session, &action, &state.gateway)
        .await
    {
        Ok(Decision::Allow) => Ok(()),
        Ok(Decision::Deny) => Err(WatchRefusal::Read(ReadRefusal::Forbidden)),
        Err(error) => Err(WatchRefusal::Read(access_refusal(error))),
    }
}

async fn admit_owned_catalogue(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
) -> Result<Admitted, WatchRefusal> {
    if state.conversations.is_none() {
        return Err(WatchRefusal::Unavailable);
    }
    authorize_owner(state, session).await?;
    let context = session.context();
    Ok(Admitted::Catalogue(CatalogueOwner {
        organization_id: context.organization_id().clone(),
        owner_id: context.principal_id().clone(),
    }))
}

async fn admit_owned_records(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    conversation: &ConversationId,
) -> Result<Admitted, WatchRefusal> {
    let service = state
        .conversations
        .as_ref()
        .ok_or(WatchRefusal::Unavailable)?;
    authorize_owner(state, session).await?;
    let caller = super::super::conversation::caller(session, "watch".to_owned());
    service
        .caller_owns(conversation, &caller)
        .await
        .map_err(ownership_refusal)?;
    Ok(Admitted::Records(conversation.clone()))
}

fn ownership_refusal(error: ConversationError) -> WatchRefusal {
    match error {
        ConversationError::NotFound | ConversationError::Deleted => {
            WatchRefusal::Read(ReadRefusal::WrongOwner)
        }
        _ => WatchRefusal::Read(ReadRefusal::Unverifiable),
    }
}

pub(in crate::product) enum WatchHandle {
    Records(CommittedChangeWatch),
    Catalogue(CatalogueChangeWatch),
}
impl WatchHandle {
    pub async fn changed(&mut self) -> Option<ChangeWatchEndReason> {
        match self {
            Self::Records(handle) => match handle.changed().await {
                ChangeWatchState::Dirty => None,
                ChangeWatchState::Closed => Some(ChangeWatchEndReason::Closed),
            },
            Self::Catalogue(handle) => match handle.changed().await {
                CatalogueWatchState::Dirty => None,
                CatalogueWatchState::Closed => Some(ChangeWatchEndReason::Closed),
                CatalogueWatchState::NotificationFailed => {
                    Some(ChangeWatchEndReason::NotificationFailed)
                }
            },
        }
    }
}
/// Why current authority refused a watch. An access error stays one, so that a
/// closing connection reports it through the socket's single mapping.
#[derive(Clone, Copy, Debug)]
pub(in crate::product) enum WatchRefusal {
    /// The session's credential, membership or browser session is no longer current.
    Access(AccessError),
    /// Admission refused: grant, receiver binding, epoch, or ownership.
    Read(ReadRefusal),
    /// The gateway has no passive-read authority composed, or the task failed.
    Unavailable,
}

impl WatchRefusal {
    /// The registration reply's code.
    pub fn code(self) -> ChangeWatchErrorCode {
        match self {
            Self::Access(error) => admission_code(access_refusal(error)),
            Self::Read(refusal) => admission_code(refusal),
            Self::Unavailable => ChangeWatchErrorCode::TemporarilyUnavailable,
        }
    }

    /// A record whose conversation is gone ends that watch. The catalogue
    /// watch on the same connection stays. Any other refusal closes it.
    pub fn ends_only_this_record(self) -> bool {
        matches!(self, Self::Read(ReadRefusal::WrongOwner))
    }

    /// How a live connection closes when a notice or a periodic check is refused.
    pub fn close_reason(self) -> SessionCloseReason {
        match self {
            Self::Access(error) => close_reason(error),
            Self::Read(ReadRefusal::Unverifiable) | Self::Unavailable => {
                SessionCloseReason::TemporaryUnavailable
            }
            Self::Read(_) => SessionCloseReason::AuthorizationLost,
        }
    }
}

fn admission_code(error: ReadRefusal) -> ChangeWatchErrorCode {
    match error {
        ReadRefusal::InvalidRequest => ChangeWatchErrorCode::InvalidRequest,
        ReadRefusal::Unauthorized => ChangeWatchErrorCode::Unauthorized,
        ReadRefusal::Forbidden => ChangeWatchErrorCode::Forbidden,
        ReadRefusal::WrongOwner => ChangeWatchErrorCode::WrongOwner,
        ReadRefusal::WrongReceiver => ChangeWatchErrorCode::WrongReceiver,
        ReadRefusal::StaleEpoch => ChangeWatchErrorCode::StaleEpoch,
        ReadRefusal::Unverifiable => ChangeWatchErrorCode::Unverifiable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nessa_protocol::product::generated::CHANGE_WATCH_ID_PATTERN;

    /// Row R6: the identities the server mints, up to the last counter, match
    /// the pattern and length the schema publishes, so the format has one owner
    /// and a schema change that disagrees with minting fails here.
    #[test]
    fn minted_identities_match_the_published_pattern_and_length() {
        let pattern = regex::Regex::new(CHANGE_WATCH_ID_PATTERN).unwrap();
        let mut tokens = WatchToken::new(Uuid::from_u128(u128::MAX));
        let first = tokens.next().unwrap();
        assert!(pattern.is_match(&first), "{first}");
        assert!(first.len() <= MAX_CHANGE_WATCH_ID_BYTES);
        tokens.accepted = u64::MAX - 1;
        let last = tokens.next().unwrap();
        assert!(pattern.is_match(&last), "{last}");
        // The published bound is exactly the longest identity minted.
        assert_eq!(last.len(), MAX_CHANGE_WATCH_ID_BYTES);
    }

    #[test]
    fn frontier_recognizes_exact_removed_ids_without_foreign_or_unminted_ids() {
        let mut tokens = WatchToken::new(Uuid::from_u128(1));
        let first = tokens.next().unwrap();
        assert!(!tokens.owns(&first));
        tokens.accepted();
        assert!(tokens.owns(&first));
        assert!(!tokens.owns(&tokens.next().unwrap()));
        assert!(!WatchToken::new(Uuid::from_u128(2)).owns(&first));
        assert!(!tokens.owns(&format!("{}-01", Uuid::from_u128(1))));
        assert!(!tokens.owns(&format!("{}-0", Uuid::from_u128(1))));
        tokens.accepted = u64::MAX - 1;
        assert!(tokens.can_accept(0));
        assert!(tokens.can_accept(1));
        assert!(!tokens.can_accept(2));
        assert_eq!(
            tokens.next().unwrap(),
            format!("{}-{}", Uuid::from_u128(1), u64::MAX)
        );
        tokens.accepted();
        assert!(tokens.can_accept(0));
        assert!(!tokens.can_accept(1));
        assert_eq!(tokens.next(), Err(ChangeWatchErrorCode::WatchCapacity));
        assert!(tokens.owns(&format!("{}-{}", Uuid::from_u128(1), u64::MAX)));
    }

    #[test]
    fn owner_params_omit_the_receiver_and_a_half_pair_is_invalid() {
        let catalogue = WatchSelector::decode(
            product_method::CONVERSATION_WATCH_CATALOGUE,
            serde_json::json!({}),
        )
        .unwrap();
        assert!(matches!(
            catalogue,
            WatchSelector::Catalogue { paired: None }
        ));
        let id = "00000000-0000-4000-8000-000000000001";
        let records = WatchSelector::decode(
            product_method::CONVERSATION_WATCH_RECORDS,
            serde_json::json!({ "conversationId": id }),
        )
        .unwrap();
        assert!(matches!(
            records,
            WatchSelector::Records { paired: None, .. }
        ));
        let paired = WatchSelector::decode(
            product_method::CONVERSATION_WATCH_RECORDS,
            serde_json::json!({
                "conversationId": id,
                "receiverId": "receiver",
                "accessEpoch": "3"
            }),
        )
        .unwrap();
        assert!(matches!(
            paired,
            WatchSelector::Records {
                paired: Some(_),
                ..
            }
        ));
        assert!(matches!(
            WatchSelector::decode(
                product_method::CONVERSATION_WATCH_CATALOGUE,
                serde_json::json!({ "receiverId": "receiver" }),
            ),
            Err(ChangeWatchErrorCode::InvalidRequest)
        ));
        assert!(matches!(
            WatchSelector::decode(
                product_method::CONVERSATION_WATCH_RECORDS,
                serde_json::json!({ "conversationId": id, "accessEpoch": "3" }),
            ),
            Err(ChangeWatchErrorCode::InvalidRequest)
        ));
    }

    #[test]
    fn a_missing_or_deleted_conversation_is_the_wrong_owner() {
        assert_eq!(
            ownership_refusal(ConversationError::NotFound).code(),
            ChangeWatchErrorCode::WrongOwner
        );
        assert_eq!(
            ownership_refusal(ConversationError::Deleted).code(),
            ChangeWatchErrorCode::WrongOwner
        );
        assert_eq!(
            ownership_refusal(ConversationError::InvalidInput).code(),
            ChangeWatchErrorCode::Unverifiable
        );
    }

    /// An owner session registers a catalogue watch and a record watch from
    /// `conversation.write` and ownership. The passive `conversation.read`
    /// action is not required, and passive read is not composed.
    #[tokio::test]
    async fn owner_session_registers_without_a_receiver() {
        use crate::agents_test_support::StubAgentProbe;
        use crate::conversation::application::{
            ConversationDependencies, ConversationLimits, ConversationRepository,
            ConversationService, ProviderSessionErasers,
        };
        use crate::conversation::domain::Conversation;
        use crate::conversation_test_support::{
            only, AcceptingCreationAudit, AcceptingDeletionAudit, AcceptingModeAudit,
            MemoryListing, MemoryRepository, MemorySummaries, Provider, ProviderFactory,
            RecordingFileLinkAudit, TestClock, DELETION_BUDGETS,
        };
        use crate::product::ProductDependencies;
        use nessa_auth::adapters::cedar::CedarPolicyEvaluator;
        use nessa_auth::application::ports::{
            AccessReader, AccessSnapshot, Clock, CredentialEvidence, CredentialVerifier,
            PortFuture, VerifiedCredential,
        };
        use nessa_auth::application::session::AuthenticateSession;
        use nessa_auth::domain::{
            AudienceId, Credential, CredentialId, Grant, Membership, MembershipId, MembershipRole,
            MembershipStatus, OrganizationId, PrincipalId, Resource, ResourceId,
        };
        use nessa_protocol::agents::AgentId;
        use nessa_protocol::clock::Clock as UptimeClock;
        use nessa_protocol::conversation::domain::{ConversationApprovalMode, ConversationModelId};
        use nessa_sdk::infrastructure::session_storage::{
            InMemoryStorage, RuntimeMessageCommitClock,
        };
        use std::sync::atomic::AtomicBool;
        use std::sync::{Arc, Mutex};

        struct Authority {
            snapshot: Mutex<AccessSnapshot>,
        }
        impl CredentialVerifier for Authority {
            fn verify<'a>(
                &'a self,
                evidence: &'a CredentialEvidence,
                audience: &'a AudienceId,
            ) -> PortFuture<'a, VerifiedCredential> {
                Box::pin(async move {
                    if evidence.expose_bytes() != b"secret" || audience.as_str() != "gateway" {
                        return Err(AccessError::InvalidCredential);
                    }
                    Ok(VerifiedCredential {
                        credential_id: CredentialId::new("credential").unwrap(),
                        expires_at: Some(200),
                    })
                })
            }
        }
        impl AccessReader for Authority {
            fn read<'a>(&'a self, _: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
                Box::pin(async move { Ok(self.snapshot.lock().unwrap().clone()) })
            }
        }
        impl Clock for Authority {
            fn unix_milliseconds(&self) -> u64 {
                100_000
            }
        }
        impl UptimeClock for Authority {
            fn elapsed_ms(&self) -> u64 {
                1
            }
        }

        let organization = OrganizationId::new("organization").unwrap();
        let principal = PrincipalId::new("principal").unwrap();
        let resource = Resource::new(
            organization.clone(),
            ResourceId::new("gateway-resource").unwrap(),
        );
        let grants = ["conversation.write", "server.read"]
            .into_iter()
            .map(|action| Grant::new(Action::new(action).unwrap(), resource.clone()))
            .collect();
        let authority = Arc::new(Authority {
            snapshot: Mutex::new(AccessSnapshot {
                credential: Credential::new(
                    CredentialId::new("credential").unwrap(),
                    principal.clone(),
                    organization.clone(),
                    AudienceId::new("gateway").unwrap(),
                    100,
                    200,
                    grants,
                )
                .unwrap(),
                membership: Membership::new(
                    MembershipId::new("membership").unwrap(),
                    principal.clone(),
                    organization.clone(),
                    MembershipRole::Member,
                    MembershipStatus::Active,
                ),
                revision: 1,
            }),
        });
        let repository = Arc::new(MemoryRepository::default());
        let summaries = Arc::new(MemorySummaries {
            summaries: Mutex::new(std::collections::HashMap::new()),
            writes: std::sync::atomic::AtomicUsize::new(0),
            load_fails: AtomicBool::new(false),
            record_fails: AtomicBool::new(false),
            erase_fails: AtomicBool::new(false),
            record_panics: AtomicBool::new(false),
            load_gate: Mutex::new(None),
        });
        let owned = ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap();
        repository
            .create(
                Conversation::new(
                    owned.clone(),
                    organization.clone(),
                    principal.clone(),
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
        let provider = Arc::new(ProviderFactory::default());
        let service = ConversationService::new(
            ConversationDependencies {
                agents: only(Arc::new(Provider::new(provider))),
                storage: Arc::new(InMemoryStorage::default()),
                message_commit_clock: Arc::new(RuntimeMessageCommitClock::new()),
                metadata: repository,
                creation_audit: Arc::new(AcceptingCreationAudit),
                mode_audit: Arc::new(AcceptingModeAudit),
                file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
                deletion_audit: Arc::new(AcceptingDeletionAudit),
                attachments: None,
                summaries: summaries.clone(),
                listing: Arc::new(MemoryListing {
                    repository: Arc::new(MemoryRepository::default()),
                    summaries,
                }),
                provider_sessions: ProviderSessionErasers::default(),
                deletion_budgets: DELETION_BUDGETS,
                clock: Arc::new(TestClock),
            },
            ConversationLimits::default(),
            None,
        )
        .unwrap();
        let state = ProductRouteState::new(
            ResourceId::new("gateway-resource").unwrap(),
            organization,
            AudienceId::new("gateway").unwrap(),
            ProductDependencies {
                verifier: authority.clone(),
                access: authority.clone(),
                clock: authority.clone(),
                policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
                uptime_clock: authority.clone(),
                agent_probe: Arc::new(StubAgentProbe::answering(false, false)),
            },
        )
        .with_conversations(Arc::new(service));
        assert!(state.passive_read.is_none());
        let session = AuthenticateSession {
            verifier: state.verifier.as_ref(),
            access: state.access.as_ref(),
            clock: state.clock.as_ref(),
        }
        .execute(
            &CredentialEvidence::new(b"secret".to_vec()).unwrap(),
            state.audience(),
        )
        .await
        .unwrap();
        let catalogue = WatchSelector::decode(
            product_method::CONVERSATION_WATCH_CATALOGUE,
            serde_json::json!({}),
        )
        .unwrap();
        let records = WatchSelector::decode(
            product_method::CONVERSATION_WATCH_RECORDS,
            serde_json::json!({ "conversationId": owned.to_string() }),
        )
        .unwrap();
        let missing = WatchSelector::decode(
            product_method::CONVERSATION_WATCH_RECORDS,
            serde_json::json!({
                "conversationId": "00000000-0000-4000-8000-000000000002"
            }),
        )
        .unwrap();
        let admitted =
            WatchSelector::recheck(&[catalogue, records, missing], &state, &session).await;
        assert!(admitted[0].is_ok(), "{:?}", admitted[0].unwrap_err().code());
        assert!(admitted[1].is_ok(), "{:?}", admitted[1].unwrap_err().code());
        assert_eq!(
            admitted[2].unwrap_err().code(),
            ChangeWatchErrorCode::WrongOwner
        );
    }
}
