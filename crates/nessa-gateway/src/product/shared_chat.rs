//! A granted chat, opened on the follower from the peer cache.
//!
//! The cache is the replica. It is not copied into this gateway's
//! conversation metadata, and a message composed here is not sent to the
//! grantor. The owner's existing `conversation.read` and `conversation.list`
//! are the view. Cedar already admitted that owner surface; this module
//! does not grant a write. The peer-write slice is what asks the grantor.

use std::sync::Arc;

use async_trait::async_trait;
use nessa_auth::application::ports::Clock as WallClock;
use nessa_client_core::retained::{
    ReadFailure, RetainedCache, RetainedCatalogueEntry, RetainedTranscript,
};
use nessa_peer_gateways::infrastructure::{PeerCommands, PeerEntry, PeerPhase, PeerRecords};
use nessa_projection::{CommittedCursor, Projection};
use nessa_protocol::clock::Clock as MonotonicClock;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::conversation::view::{
    ConversationCapabilities, ConversationSelectionView, ConversationTranscriptState,
    ConversationView,
};
use nessa_protocol::product::generated::ConversationSummary;
use nessa_sync::replication::catalogue::{EntryKey, MAX_CATALOGUE_ENTRIES};
use tokio::task::spawn_blocking;
use uuid::Uuid;

use super::state::ProductRouteState;
use crate::conversation::application::{load_conversation, ConversationError};

/// One saved chat, as the retained fold already projects it.
pub struct OpenedReplica {
    pub view: ConversationView,
    pub cursor: CommittedCursor,
}

/// The follower's copy of chats a peer has granted.
#[async_trait]
pub trait SharedChats: Send + Sync {
    /// Chats still in the cache. `archived` selects the archived list.
    async fn list(&self, archived: bool) -> Result<Vec<ConversationSummary>, ConversationError>;
    /// The saved transcript, or nothing when this id is not a granted chat.
    async fn open(&self, id: &ConversationId) -> Result<Option<OpenedReplica>, ConversationError>;
    /// Whether the cache still holds `id`.
    async fn holds(&self, id: &ConversationId) -> Result<bool, ConversationError>;
}

/// The peer records this gateway already keeps.
pub struct PeerCacheChats {
    peers: Arc<PeerCommands>,
    wall: Arc<dyn WallClock>,
    monotonic: Arc<dyn MonotonicClock>,
}

impl PeerCacheChats {
    pub fn new(
        peers: Arc<PeerCommands>,
        wall: Arc<dyn WallClock>,
        monotonic: Arc<dyn MonotonicClock>,
    ) -> Self {
        Self {
            peers,
            wall,
            monotonic,
        }
    }
}

struct ActivePeer {
    key: nessa_auth::domain::pairing::DeviceKey,
    receiver: String,
}

#[async_trait]
impl SharedChats for PeerCacheChats {
    async fn list(&self, archived: bool) -> Result<Vec<ConversationSummary>, ConversationError> {
        let active = active_peers(&self.peers).await?;
        let records = Arc::clone(self.peers.records());
        let wall = Arc::clone(&self.wall);
        let monotonic = Arc::clone(&self.monotonic);
        spawn_blocking(move || list_cached(&records, &active, wall, monotonic, archived))
            .await
            .map_err(|_| ConversationError::Unavailable)?
    }

    async fn open(&self, id: &ConversationId) -> Result<Option<OpenedReplica>, ConversationError> {
        self.lookup(id, true).await
    }

    async fn holds(&self, id: &ConversationId) -> Result<bool, ConversationError> {
        Ok(self.lookup(id, false).await?.is_some())
    }
}

impl PeerCacheChats {
    async fn lookup(
        &self,
        id: &ConversationId,
        project: bool,
    ) -> Result<Option<OpenedReplica>, ConversationError> {
        let active = active_peers(&self.peers).await?;
        let records = Arc::clone(self.peers.records());
        let wall = Arc::clone(&self.wall);
        let monotonic = Arc::clone(&self.monotonic);
        let id = id.clone();
        spawn_blocking(move || find_cached(&records, &active, wall, monotonic, &id, project))
            .await
            .map_err(|_| ConversationError::Unavailable)?
    }
}

/// The stand-in when a test set one, otherwise the live peer cache.
pub(super) fn source(state: &ProductRouteState) -> Option<Arc<dyn SharedChats>> {
    if let Some(chats) = &state.linking.shared_chats {
        return Some(Arc::clone(chats));
    }
    let peers = state.linking.peers.clone()?;
    Some(Arc::new(PeerCacheChats::new(
        peers,
        Arc::clone(&state.clock),
        Arc::clone(&state.uptime_clock),
    )))
}

/// The saved view when `id` is not one of this gateway's own conversations
/// and Cedar allows `session` to read the local catalogue owner's chats.
pub(super) async fn open_replica(
    state: &ProductRouteState,
    session: &nessa_auth::application::session::AuthenticatedSession,
    id: &ConversationId,
) -> Result<Option<OpenedReplica>, ConversationError> {
    if local_conversation(state, id).await? || !catalogue_admitted(state, session, false).await? {
        return Ok(None);
    }
    let Some(chats) = source(state) else {
        return Ok(None);
    };
    chats.open(id).await
}

/// Refuse a write on a granted chat the caller may read. A local conversation
/// is left to the service. Someone Cedar does not allow is told nothing:
/// the command falls through and is not found. A cache that cannot be read
/// is unavailable, so the write is not saved here while the grant is unknown.
pub(super) async fn refuse_write(
    state: &ProductRouteState,
    session: &nessa_auth::application::session::AuthenticatedSession,
    id: &ConversationId,
) -> Result<(), ConversationError> {
    if local_conversation(state, id).await? || !catalogue_admitted(state, session, false).await? {
        return Ok(());
    }
    let Some(chats) = source(state) else {
        return Ok(());
    };
    if chats.holds(id).await? {
        return Err(ConversationError::CapabilitiesRefused {
            policies: Vec::new(),
        });
    }
    Ok(())
}

/// Whether Cedar's `owner.list` or `owner.read` allows this caller the local
/// catalogue. No catalogue owner on the route means the cache is not shown.
pub(super) async fn catalogue_admitted(
    state: &ProductRouteState,
    session: &nessa_auth::application::session::AuthenticatedSession,
    list: bool,
) -> Result<bool, ConversationError> {
    let Some((organization, owner)) = state.reads.catalogue_owner.clone() else {
        return Ok(false);
    };
    let Some(service) = state.conversation.service.as_ref() else {
        return Ok(false);
    };
    let caller = super::conversation::caller(session, String::new());
    if list {
        service.admits_shared_list(&caller, &organization, &owner)
    } else {
        service.admits_shared_read(&caller, &organization, &owner)
    }
}

/// Add `replicas` that fit in `limit`. A remainder keeps the page incomplete.
pub(super) fn append_bounded(
    rows: &mut Vec<ConversationSummary>,
    replicas: Vec<ConversationSummary>,
    limit: usize,
    complete: &mut bool,
) {
    let replicas = replicas
        .into_iter()
        .filter(|replica| {
            !rows
                .iter()
                .any(|row| row.conversation_id == replica.conversation_id)
        })
        .collect::<Vec<_>>();
    let room = limit.saturating_sub(rows.len());
    if replicas.len() <= room {
        rows.extend(replicas);
        return;
    }
    rows.extend(replicas.into_iter().take(room));
    *complete = false;
}

async fn local_conversation(
    state: &ProductRouteState,
    id: &ConversationId,
) -> Result<bool, ConversationError> {
    let Some((_, repository, _)) = state.reads.passive_read.as_ref() else {
        return Ok(false);
    };
    Ok(load_conversation(repository.as_ref(), id).await?.is_some())
}

async fn active_peers(peers: &PeerCommands) -> Result<Vec<ActivePeer>, ConversationError> {
    let listed = peers
        .list()
        .await
        .map_err(|_| ConversationError::Unavailable)?;
    Ok(listed
        .into_iter()
        .filter_map(|(entry, _)| {
            let PeerEntry::Readable(record) = entry else {
                return None;
            };
            let PeerPhase::Active { receiver, .. } = record.phase() else {
                return None;
            };
            Some(ActivePeer {
                key: *record.key(),
                receiver: receiver.as_str().to_owned(),
            })
        })
        .collect())
}

fn list_cached(
    records: &PeerRecords,
    peers: &[ActivePeer],
    wall: Arc<dyn WallClock>,
    monotonic: Arc<dyn MonotonicClock>,
    archived: bool,
) -> Result<Vec<ConversationSummary>, ConversationError> {
    let mut rows = Vec::new();
    for peer in peers {
        match peer_rows(records, peer, &wall, &monotonic, archived) {
            Ok(found) => rows.extend(found),
            Err(error) => {
                tracing::warn!(
                    receiver = %peer.receiver,
                    error = ?error,
                    "a peer cache was left out of the conversation list"
                );
            }
        }
    }
    Ok(rows)
}

fn find_cached(
    records: &PeerRecords,
    peers: &[ActivePeer],
    wall: Arc<dyn WallClock>,
    monotonic: Arc<dyn MonotonicClock>,
    id: &ConversationId,
    project: bool,
) -> Result<Option<OpenedReplica>, ConversationError> {
    let mut failed = false;
    for peer in peers {
        match peer_open(records, peer, &wall, &monotonic, id, project) {
            Ok(Some(opened)) => return Ok(Some(opened)),
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(
                    receiver = %peer.receiver,
                    error = ?error,
                    "a peer cache could not be read"
                );
                failed = true;
            }
        }
    }
    if failed {
        Err(ConversationError::Unavailable)
    } else {
        Ok(None)
    }
}

fn peer_rows(
    records: &PeerRecords,
    peer: &ActivePeer,
    wall: &Arc<dyn WallClock>,
    monotonic: &Arc<dyn MonotonicClock>,
    archived: bool,
) -> Result<Vec<ConversationSummary>, ReadFailure> {
    let mut cache = open_cache(records, peer, wall, monotonic)?;
    let Some(scope) = cache.saved_catalogue_scope(&peer.receiver)? else {
        return Ok(Vec::new());
    };
    let origin = scope.origin().as_str().to_owned();
    let stream = scope.stream().as_str().to_owned();
    let mut after: Option<EntryKey> = None;
    let mut rows = Vec::new();
    while let Some(page) = cache.catalogue_page(
        &peer.receiver,
        &origin,
        &stream,
        after.as_ref(),
        MAX_CATALOGUE_ENTRIES,
    )? {
        for entry in page.entries() {
            if let Some(row) = summary_of(entry, archived) {
                rows.push(row);
            }
        }
        let Some(next) = page.next().cloned() else {
            break;
        };
        after = Some(next);
    }
    Ok(rows)
}

fn summary_of(entry: &RetainedCatalogueEntry, archived: bool) -> Option<ConversationSummary> {
    if entry.manifest().deleted {
        return None;
    }
    let metadata = entry.metadata()?;
    let summary = metadata.summary();
    let is_archived = summary.is_some_and(|summary| summary.archived());
    if is_archived != archived {
        return None;
    }
    Some(ConversationSummary {
        conversation_id: metadata.id().to_string(),
        title: summary
            .and_then(|summary| summary.title())
            .map(|title| title.as_str().to_owned()),
        preview: summary
            .and_then(|summary| summary.preview())
            .map(|preview| preview.as_str().to_owned()),
        created_at_ms: metadata.created_at_ms(),
        updated_at_ms: summary
            .map(|summary| summary.updated_at_ms())
            .unwrap_or_else(|| metadata.created_at_ms()),
        running: false,
        archived: is_archived,
        channel_id: None,
    })
}

fn peer_open(
    records: &PeerRecords,
    peer: &ActivePeer,
    wall: &Arc<dyn WallClock>,
    monotonic: &Arc<dyn MonotonicClock>,
    id: &ConversationId,
    project: bool,
) -> Result<Option<OpenedReplica>, ReadFailure> {
    let mut cache = open_cache(records, peer, wall, monotonic)?;
    let Some(scope) = cache.saved_catalogue_scope(&peer.receiver)? else {
        return Ok(None);
    };
    let held = cache.conversation_ids(&peer.receiver)?;
    let name = id.to_string();
    if !held.iter().any(|held| held == &name) {
        return Ok(None);
    }
    if !project {
        return Ok(Some(placeholder(id)));
    }
    let origin = scope.origin().as_str().to_owned();
    let stream = scope.stream().as_str().to_owned();
    let incarnation = scope.incarnation().as_str().to_owned();
    let selection = selection_for(&mut cache, &peer.receiver, &origin, &stream, id)?;
    let transcript = cache.transcript_view(&peer.receiver, &origin, id, Uuid::nil())?;
    if matches!(transcript, RetainedTranscript::Deleted) {
        return Ok(None);
    }
    Ok(Some(project_transcript(
        id,
        incarnation,
        selection,
        transcript,
    )))
}

fn open_cache(
    records: &PeerRecords,
    peer: &ActivePeer,
    wall: &Arc<dyn WallClock>,
    monotonic: &Arc<dyn MonotonicClock>,
) -> Result<RetainedCache, ReadFailure> {
    RetainedCache::open(
        &records.cache_path(&peer.key),
        Arc::clone(wall),
        Arc::clone(monotonic),
    )
}

fn selection_for(
    cache: &mut RetainedCache,
    receiver: &str,
    origin: &str,
    stream: &str,
    id: &ConversationId,
) -> Result<Option<ConversationSelectionView>, ReadFailure> {
    let mut after: Option<EntryKey> = None;
    while let Some(page) = cache.catalogue_page(
        receiver,
        origin,
        stream,
        after.as_ref(),
        MAX_CATALOGUE_ENTRIES,
    )? {
        for entry in page.entries() {
            let Some(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.id() != id {
                continue;
            }
            let Some(agent) = metadata.agent() else {
                return Ok(None);
            };
            return Ok(Some(ConversationSelectionView {
                agent,
                model: metadata.model().as_str().to_owned(),
                approval_mode: metadata.approval_mode(),
            }));
        }
        let Some(next) = page.next().cloned() else {
            return Ok(None);
        };
        after = Some(next);
    }
    Ok(None)
}

fn project_transcript(
    id: &ConversationId,
    incarnation: String,
    selection: Option<ConversationSelectionView>,
    transcript: RetainedTranscript,
) -> OpenedReplica {
    match transcript {
        RetainedTranscript::Retained(snapshot) => {
            let progress = snapshot.progress();
            let mut view = snapshot.view().clone();
            if view.selection.is_none() {
                view.selection = selection;
            }
            OpenedReplica {
                view,
                cursor: CommittedCursor {
                    incarnation: progress.scope().incarnation().as_str().to_owned(),
                    position: progress.applied(),
                    observed_head: progress.downloaded().max(progress.applied()),
                    read_through: progress.applied(),
                },
            }
        }
        RetainedTranscript::NotLoaded => {
            let mut projection =
                Projection::new(id.to_string(), ConversationCapabilities::read_only(), None);
            projection.transcript_state(ConversationTranscriptState::NotLoaded);
            if let Some(selection) = selection {
                projection.selection(selection);
            }
            OpenedReplica {
                view: projection.read(),
                cursor: CommittedCursor {
                    incarnation,
                    position: 0,
                    observed_head: 0,
                    read_through: 0,
                },
            }
        }
        // `peer_open` answers a deleted transcript before this match.
        RetainedTranscript::Deleted => OpenedReplica {
            view: Projection::new(id.to_string(), ConversationCapabilities::read_only(), None)
                .read(),
            cursor: CommittedCursor {
                incarnation,
                position: 0,
                observed_head: 0,
                read_through: 0,
            },
        },
    }
}

/// A membership answer. The caller asked only whether the id is held.
fn placeholder(id: &ConversationId) -> OpenedReplica {
    OpenedReplica {
        view: Projection::new(id.to_string(), ConversationCapabilities::read_only(), None).read(),
        cursor: CommittedCursor {
            incarnation: String::new(),
            position: 0,
            observed_head: 0,
            read_through: 0,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use nessa_auth::adapters::cedar::CedarPolicyEvaluator;
    use nessa_auth::adapters::local::{BootstrapRequest, LocalCredentialStore};
    use nessa_auth::application::dto::{
        CredentialGrantDto, MembershipInputDto, MembershipRoleDto, MembershipStateDto,
        OrganizationInputDto, PrincipalInputDto, PrincipalKindDto, ResourceDto,
    };
    use nessa_auth::application::ports::Clock;
    use nessa_auth::application::session::AuthenticateSession;
    use nessa_auth::domain::{AudienceId, OrganizationId, ResourceId};
    use nessa_projection::{CommittedCursor, Projection};
    use nessa_protocol::agents::AgentId;
    use nessa_protocol::conversation::domain::{ConversationApprovalMode, ConversationId};
    use nessa_protocol::conversation::view::{
        ConversationCapabilities, ConversationSelectionView, ConversationTranscriptState,
        ConversationView,
    };
    use nessa_protocol::product::generated::ConversationSummary;
    use nessa_protocol::protocol::{OutgoingMessage, RequestFrame, ResponseFrame};
    use nessa_sdk::domain::agent_execution::executions::Initiation;
    use serde_json::json;

    use super::{OpenedReplica, SharedChats};
    use crate::app::dependencies::RuntimeDependencies;
    use crate::conversation::application::ConversationLimits;
    use crate::product::conversation::dispatch;
    use crate::product::{ProductDependencies, ProductRouteState};

    struct Held {
        id: String,
        view: ConversationView,
        cursor: CommittedCursor,
    }

    #[async_trait]
    impl SharedChats for Held {
        async fn list(
            &self,
            archived: bool,
        ) -> Result<Vec<ConversationSummary>, crate::conversation::application::ConversationError>
        {
            if archived {
                return Ok(Vec::new());
            }
            Ok(vec![ConversationSummary {
                conversation_id: self.id.clone(),
                title: self.view.title.clone(),
                preview: Some("hello from the grantor".into()),
                created_at_ms: 10,
                updated_at_ms: 11,
                running: false,
                archived: false,
                channel_id: None,
            }])
        }

        async fn open(
            &self,
            id: &ConversationId,
        ) -> Result<Option<OpenedReplica>, crate::conversation::application::ConversationError>
        {
            if id.to_string() != self.id {
                return Ok(None);
            }
            Ok(Some(OpenedReplica {
                view: self.view.clone(),
                cursor: self.cursor.clone(),
            }))
        }

        async fn holds(
            &self,
            id: &ConversationId,
        ) -> Result<bool, crate::conversation::application::ConversationError> {
            Ok(id.to_string() == self.id)
        }
    }

    struct Wall;
    impl Clock for Wall {
        fn unix_milliseconds(&self) -> u64 {
            1_700_000_000_000
        }
        fn unix_seconds(&self) -> u64 {
            1_700_000_000
        }
    }

    struct NoAgents;
    impl crate::agents::application::AgentProbe for NoAgents {
        fn evidence(&self, _: AgentId) -> Option<crate::agents::application::AgentProbeEvidence> {
            None
        }
    }

    async fn ask(
        state: &ProductRouteState,
        session: &nessa_auth::application::session::AuthenticatedSession,
        method: &str,
        params: serde_json::Value,
    ) -> ResponseFrame {
        response(
            dispatch(
                state,
                session,
                RequestFrame::new("request", method, &params).unwrap(),
                Initiation::owner(),
            )
            .await,
        )
    }

    fn response(message: OutgoingMessage) -> ResponseFrame {
        match message {
            OutgoingMessage::Response(frame) => frame,
            OutgoingMessage::Event(frame) => panic!("unexpected event {}", frame.event),
        }
    }

    async fn owner_state(
        chats: Arc<dyn SharedChats>,
    ) -> (
        ProductRouteState,
        nessa_auth::application::session::AuthenticatedSession,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("auth");
        nessa_local_storage::create_directory(&root).unwrap();
        let policy = Arc::new(CedarPolicyEvaluator::new().unwrap());
        let auth = Arc::new(
            LocalCredentialStore::open(&root, "credentials.json", policy.clone()).unwrap(),
        );
        let grant = |action: &str| CredentialGrantDto {
            action: action.into(),
            resource: ResourceDto {
                organization_id: "org".into(),
                id: "gateway".into(),
            },
        };
        let issued = auth
            .bootstrap(BootstrapRequest {
                gateway_id: "gateway".into(),
                organization: OrganizationInputDto { id: "org".into() },
                principal: PrincipalInputDto {
                    id: "owner".into(),
                    kind: PrincipalKindDto::Human,
                },
                membership: MembershipInputDto {
                    id: "member".into(),
                    principal_id: "owner".into(),
                    organization_id: "org".into(),
                    role: MembershipRoleDto::Admin,
                    state: MembershipStateDto::Active,
                },
                credential_id: "credential".into(),
                issued_at: Wall.unix_seconds(),
                expires_at: None,
                grants: ["conversation.read", "conversation.write", "server.read"]
                    .into_iter()
                    .map(grant)
                    .collect(),
            })
            .unwrap();
        let session = AuthenticateSession {
            policy: &CedarPolicyEvaluator::new().unwrap(),
            verifier: auth.as_ref(),
            access: auth.as_ref(),
            clock: &Wall,
        }
        .execute(&issued.evidence, &AudienceId::new("gateway").unwrap())
        .await
        .unwrap();
        let (service, _, _, _) =
            crate::conversation_test_support::fixture(ConversationLimits::default());
        let catalog = serde_json::from_value(json!({
            "environments": [],
            "commandHosts": [],
            "agents": [{
                "agent": "claude",
                "defaultModel": "model",
                "models": [{
                    "modelId": "model",
                    "displayName": "Test model",
                    "maxContextWindowTokens": 100000,
                    "reasoning": false,
                    "imageInput": false,
                    "approvalModes": [{
                        "id": "ask",
                        "name": "Provider asks",
                        "description": "The provider requests approval where required."
                    }]
                }]
            }],
            "nativeHooks": {"claude": "off", "codex": "off"}
        }))
        .unwrap();
        let state = ProductRouteState::new(
            ResourceId::new("gateway").unwrap(),
            OrganizationId::new("org").unwrap(),
            AudienceId::new("gateway").unwrap(),
            ProductDependencies {
                owner_phone_purpose: auth.clone(),
                owner_phone_policy: Arc::new(
                    crate::conversation::infrastructure::CedarOwnerPhonePolicyEvaluator::new()
                        .unwrap(),
                ),
                owner_phone_receivers: None,
                verifier: auth.clone(),
                access: auth,
                clock: Arc::new(Wall),
                policy: policy.clone(),
                uptime_clock: RuntimeDependencies::default().clock,
                agent_probe: Arc::new(NoAgents),
            },
        )
        .with_conversations(Arc::new(service))
        .with_agents_catalog(catalog)
        .with_catalogue_owner(
            OrganizationId::new("org").unwrap(),
            nessa_auth::domain::PrincipalId::new("owner").unwrap(),
        )
        .with_shared_chats(chats);
        // The registry directory has to outlive the store.
        std::mem::forget(directory);
        (state, session)
    }

    fn replica() -> Arc<Held> {
        let id = "00000000-0000-4000-8000-000000000890";
        let mut projection =
            Projection::new(id.into(), ConversationCapabilities::read_only(), None);
        projection.selection(ConversationSelectionView {
            agent: AgentId::Claude,
            model: "model".into(),
            approval_mode: ConversationApprovalMode::Ask,
        });
        projection.transcript_state(ConversationTranscriptState::CompleteEmpty);
        let mut view = projection.read();
        view.title = Some("from the grantor".into());
        view.messages.clear();
        Arc::new(Held {
            id: id.into(),
            view,
            cursor: CommittedCursor {
                incarnation: "1".into(),
                position: 1,
                observed_head: 1,
                read_through: 1,
            },
        })
    }

    #[tokio::test]
    async fn the_owner_reads_a_granted_chat_and_cannot_write_it() {
        let held = replica();
        let id = held.id.clone();
        let (state, session) = owner_state(held).await;
        let read = ask(
            &state,
            &session,
            "conversation.read",
            json!({ "conversationId": id }),
        )
        .await;
        assert!(read.ok, "{read:?}");
        let payload = read.payload.unwrap();
        assert_eq!(payload["conversationId"], id);
        assert_eq!(payload["title"], "from the grantor");
        assert_eq!(payload["capabilities"]["queue"], false);
        assert_eq!(payload["messages"], json!([]));

        let listed = ask(&state, &session, "conversation.list", json!({})).await;
        assert!(listed.ok, "{listed:?}");
        let rows = listed.payload.unwrap()["conversations"]
            .as_array()
            .unwrap()
            .clone();
        assert!(rows.iter().any(|row| row["conversationId"] == id));

        let missing = ask(
            &state,
            &session,
            "conversation.read",
            json!({ "conversationId": "00000000-0000-4000-8000-000000000891" }),
        )
        .await;
        assert!(!missing.ok);
        assert_eq!(missing.error.unwrap().code, "conversation_not_found");

        let sent = ask(
            &state,
            &session,
            "conversation.send",
            json!({
                "conversationId": id,
                "requestId": "send-1",
                "executionId": "turn-1",
                "text": "a note composed on the follower",
                "attachments": [],
                "files": []
            }),
        )
        .await;
        assert!(!sent.ok);
        assert_eq!(sent.error.unwrap().code, "capabilities_refused");

        let created = ask(
            &state,
            &session,
            "conversation.create",
            json!({ "conversationId": id, "requestId": "create-1" }),
        )
        .await;
        assert!(!created.ok);
        assert_eq!(created.error.unwrap().code, "capabilities_refused");
    }

    #[tokio::test]
    async fn another_principal_is_not_told_the_granted_chat_exists() {
        let held = replica();
        let id = held.id.clone();
        let (mut state, session) = owner_state(held).await;
        state = state.with_catalogue_owner(
            OrganizationId::new("org").unwrap(),
            nessa_auth::domain::PrincipalId::new("other").unwrap(),
        );
        let read = ask(
            &state,
            &session,
            "conversation.read",
            json!({ "conversationId": id }),
        )
        .await;
        assert!(!read.ok);
        assert_eq!(read.error.unwrap().code, "conversation_not_found");
        let listed = ask(&state, &session, "conversation.list", json!({})).await;
        assert!(listed.ok, "{listed:?}");
        let rows = listed.payload.unwrap()["conversations"]
            .as_array()
            .unwrap()
            .clone();
        assert!(rows.iter().all(|row| row["conversationId"] != id));
        let missing = ask(
            &state,
            &session,
            "conversation.send",
            json!({
                "conversationId": "00000000-0000-4000-8000-000000000891",
                "requestId": "send-missing",
                "executionId": "turn-missing",
                "text": "not a chat",
                "attachments": [],
                "files": []
            }),
        )
        .await;
        let sent = ask(
            &state,
            &session,
            "conversation.send",
            json!({
                "conversationId": id,
                "requestId": "send-2",
                "executionId": "turn-2",
                "text": "not this principal",
                "attachments": [],
                "files": []
            }),
        )
        .await;
        assert!(!sent.ok);
        assert!(!missing.ok);
        assert_eq!(sent.error.unwrap().code, missing.error.unwrap().code);
    }

    #[test]
    fn a_full_catalogue_page_does_not_grow_past_its_bound() {
        let summary = |id: &str| ConversationSummary {
            conversation_id: id.to_owned(),
            title: None,
            preview: None,
            created_at_ms: 1,
            updated_at_ms: 1,
            running: false,
            archived: false,
            channel_id: None,
        };
        let mut rows: Vec<_> = (0..256)
            .map(|index| summary(&format!("local-{index}")))
            .collect();
        let mut complete = true;
        super::append_bounded(&mut rows, vec![summary("shared")], 256, &mut complete);
        assert_eq!(rows.len(), 256);
        assert!(!complete);
        let mut room = vec![summary("local")];
        complete = true;
        super::append_bounded(&mut room, vec![summary("shared")], 256, &mut complete);
        assert_eq!(room.len(), 2);
        assert!(complete);
    }
}
