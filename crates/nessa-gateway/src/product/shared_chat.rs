//! A granted chat, opened on the follower from the peer cache.
//!
//! The cache is the replica. It is not copied into this gateway's
//! conversation metadata, and a message composed here is not sent to the
//! grantor. The owner's existing `conversation.read` and `conversation.list`
//! are the view. Cedar already admitted that owner surface; this module
//! does not grant a write. The peer-write slice is what asks the grantor.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use nessa_auth::application::ports::Clock as WallClock;
use nessa_client_core::retained::{
    EarlierMiss, PeerArtifact, ReadFailure, RetainedCache, RetainedCatalogueEntry,
    RetainedTranscript,
};
use nessa_peer_gateways::infrastructure::{
    read_granted_artifact, PeerCommands, PeerEntry, PeerPhase, PeerRecord, PeerRecords,
};
use nessa_projection::{CommittedCursor, Projection};
use nessa_protocol::clock::Clock as MonotonicClock;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::conversation::view::{
    ConversationCapabilities, ConversationEarlier, ConversationPoint, ConversationSelectionView,
    ConversationTranscriptState, ConversationView,
};
use nessa_protocol::product::generated::{ConversationObserveCursor, ConversationSummary};
use nessa_protocol::product_contract::generated::ConversationErrorCode;
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
    /// [`SharedCatalogue::complete`] is false when a peer cache could not be
    /// read: the chats it holds are not known to be absent.
    async fn list(&self, archived: bool) -> Result<SharedCatalogue, ConversationError>;
    /// The saved transcript, or nothing when this id is not a granted chat.
    async fn open(&self, id: &ConversationId) -> Result<Option<OpenedReplica>, ConversationError>;
    /// Whether the cache still holds `id`.
    async fn holds(&self, id: &ConversationId) -> Result<bool, ConversationError>;
    /// Turns before `before` when `id` is a retained chat. `Ok(None)` when it
    /// is not. A point the history does not contain is invalid input.
    async fn earlier(
        &self,
        id: &ConversationId,
        before: &ConversationPoint,
        after: Option<&ConversationPoint>,
    ) -> Result<Option<ConversationEarlier>, ConversationError>;
    /// One range of a file `id` holds on the peer that granted it.
    /// [`SharedFileRead::Elsewhere`] when this cache does not hold `id`.
    async fn read_file(
        &self,
        id: &ConversationId,
        file: SharedFileRequest,
    ) -> Result<SharedFileRead, ConversationError>;
}

/// The shared chats one list read found.
pub struct SharedCatalogue {
    /// Chats whose caches were read, in peer order. The first peer to name an
    /// id is the one a later open of that id uses.
    pub rows: Vec<ConversationSummary>,
    /// Every active peer cache was read.
    pub complete: bool,
}

/// A file range the transcript named, as `artifact.read` asks it.
pub struct SharedFileRequest {
    /// The digest the view carries.
    pub digest: String,
    /// The media type the view carries.
    pub media_type: String,
    /// The size the view carries, with the digest and media type.
    pub size: u64,
    /// Where the range starts.
    pub offset: u64,
    /// How many bytes the range asks for.
    pub max_bytes: usize,
}

/// What a shared-file range read found.
pub enum SharedFileRead {
    /// `id` is not a chat this cache holds.
    Elsewhere,
    /// The peer returned this range.
    Range(Vec<u8>),
    /// The peer holds the chat and does not hold this file.
    NotHeld,
    /// The chat is gone on the peer.
    Missing,
    /// The peer could not be asked now.
    Unavailable,
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
    async fn list(&self, archived: bool) -> Result<SharedCatalogue, ConversationError> {
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

    async fn earlier(
        &self,
        id: &ConversationId,
        before: &ConversationPoint,
        after: Option<&ConversationPoint>,
    ) -> Result<Option<ConversationEarlier>, ConversationError> {
        let active = active_peers(&self.peers).await?;
        let records = Arc::clone(self.peers.records());
        let wall = Arc::clone(&self.wall);
        let monotonic = Arc::clone(&self.monotonic);
        let id = id.clone();
        let before = before.clone();
        let after = after.cloned();
        spawn_blocking(move || {
            find_earlier(
                &records,
                &active,
                wall,
                monotonic,
                &id,
                &before,
                after.as_ref(),
            )
        })
        .await
        .map_err(|_| ConversationError::Unavailable)?
    }

    async fn read_file(
        &self,
        id: &ConversationId,
        file: SharedFileRequest,
    ) -> Result<SharedFileRead, ConversationError> {
        let active = active_peers(&self.peers).await?;
        let records = Arc::clone(self.peers.records());
        let wall = Arc::clone(&self.wall);
        let monotonic = Arc::clone(&self.monotonic);
        let id = id.clone();
        spawn_blocking(move || read_cached_file(&records, &active, wall, monotonic, &id, &file))
            .await
            .map_err(|_| ConversationError::Unavailable)?
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

/// The shared-chat port composition injected. Production injects the peer
/// cache. A test injects a stand-in through the same field.
pub(super) fn source(state: &ProductRouteState) -> Option<Arc<dyn SharedChats>> {
    state.linking.shared_chats.clone()
}

/// One range of a file a granted chat names, read from the peer that granted
/// it. Cedar's `owner.read` admits the catalogue owner. A local conversation,
/// or a caller Cedar does not admit, is [`SharedFileRead::Elsewhere`] so the
/// local hold answers instead. Nothing is written into this gateway's store.
pub(super) async fn read_shared_file(
    state: &ProductRouteState,
    session: &nessa_auth::application::session::AuthenticatedSession,
    id: &ConversationId,
    file: SharedFileRequest,
) -> Result<SharedFileRead, ConversationError> {
    if local_conversation(state, id).await? || !catalogue_admitted(state, session, false).await? {
        return Ok(SharedFileRead::Elsewhere);
    }
    let Some(chats) = source(state) else {
        return Ok(SharedFileRead::Elsewhere);
    };
    chats.read_file(id, file).await
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

/// Turns before `before` when `id` is a granted chat the caller may read.
/// `Ok(None)` when it is not one, so the local history answers instead.
pub(super) async fn read_earlier(
    state: &ProductRouteState,
    session: &nessa_auth::application::session::AuthenticatedSession,
    id: &ConversationId,
    before: &ConversationPoint,
    after: Option<&ConversationPoint>,
) -> Result<Option<ConversationEarlier>, ConversationError> {
    if local_conversation(state, id).await? || !catalogue_admitted(state, session, false).await? {
        return Ok(None);
    }
    let Some(chats) = source(state) else {
        return Ok(None);
    };
    chats.earlier(id, before, after).await
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

/// Add `replicas` that fit in `limit`, after `skip` of them that an earlier
/// page of this walk already returned. `Some` is where the next page starts
/// when this one could not hold the rest. A remainder keeps the page incomplete.
pub(super) fn append_bounded(
    rows: &mut Vec<ConversationSummary>,
    replicas: Vec<ConversationSummary>,
    limit: usize,
    complete: &mut bool,
    skip: usize,
) -> Option<u64> {
    // The first peer to name an id wins, the same order `list_cached` and
    // `find_cached` walk. A later peer's copy of that id is not a second row.
    let mut seen = HashSet::new();
    let mut replicas = replicas
        .into_iter()
        .filter(|replica| {
            seen.insert(replica.conversation_id.clone())
                && !rows
                    .iter()
                    .any(|row| row.conversation_id == replica.conversation_id)
        })
        .collect::<Vec<_>>();
    let rest = if skip >= replicas.len() {
        Vec::new()
    } else {
        replicas.split_off(skip)
    };
    let room = limit.saturating_sub(rows.len());
    if rest.len() <= room {
        rows.extend(rest);
        return None;
    }
    let placed = room;
    rows.extend(rest.into_iter().take(placed));
    *complete = false;
    Some(skip.saturating_add(placed) as u64)
}

/// Append one shared-catalogue read. A peer cache that could not be read keeps
/// the page incomplete even when every row that was read fits.
/// Where a shared page stopped, and the fingerprint of the list it walked.
pub(super) struct SharedAppend {
    pub next: Option<u64>,
    pub fingerprint: u32,
    /// Every shared chat this filter includes, when the cache was read and
    /// the set fits the list field. `None` when it was not read, the read
    /// was incomplete, or the set is longer than the field can carry.
    pub shared_ids: Option<Vec<String>>,
}

/// The shared ids a list may name. The wire field holds at most
/// [`MAX_CATALOGUE_ENTRIES`]. A longer set is left off: cutting it would
/// look like the chats that did not fit had been withdrawn.
pub(super) fn shared_membership(ids: Vec<String>) -> Option<Vec<String>> {
    if ids.len() > MAX_CATALOGUE_ENTRIES {
        None
    } else {
        Some(ids)
    }
}

pub(super) fn append_catalogue(
    rows: &mut Vec<ConversationSummary>,
    catalogue: SharedCatalogue,
    limit: usize,
    complete: &mut bool,
    skip: usize,
    expected: Option<u32>,
) -> SharedAppend {
    if !catalogue.complete {
        *complete = false;
    }
    let fingerprint = replica_fingerprint(&catalogue.rows);
    // The offset names a position in the list that minted it. A changed
    // list is not a later page of the walk the caller already accumulated:
    // that walk stays incomplete and offers no cursor, so it cannot claim
    // the withdrawn chat is still a member.
    if matches!(expected, Some(seen) if seen != fingerprint) {
        *complete = false;
        return SharedAppend {
            next: None,
            fingerprint,
            shared_ids: None,
        };
    }
    SharedAppend {
        next: append_bounded(rows, catalogue.rows, limit, complete, skip),
        fingerprint,
        shared_ids: None,
    }
}

/// The replica list a shared-page cursor was issued against, and how far
/// into that list the next page starts.
pub(super) struct SharedMark {
    /// [`replica_fingerprint`] of the list when the cursor was minted.
    pub fingerprint: u32,
    pub offset: usize,
}

/// The mark in a cursor minted by [`shared_page_cursor`]. The id is a
/// canonical UUID whose middle run [`ConversationId`] refuses, so a local
/// catalogue cursor, which is the last conversation's id, cannot carry it.
pub(super) fn shared_page_mark(cursor: &ConversationObserveCursor) -> Option<SharedMark> {
    let (fingerprint, offset) = cursor.id.split_once("-ffff-ffff-ffff-")?;
    if fingerprint.len() != 8 || offset.len() != 12 {
        return None;
    }
    Some(SharedMark {
        fingerprint: u32::from_str_radix(fingerprint, 16).ok()?,
        offset: usize::try_from(u64::from_str_radix(offset, 16).ok()?).ok()?,
    })
}

/// A cursor the desktop will walk after `base`. `fingerprint` is the replica
/// list it continues, and `offset` is where the next page starts. No `base`
/// is the first observe of a walk.
pub(super) fn shared_page_cursor(
    base: Option<&ConversationObserveCursor>,
    offset: u64,
    fingerprint: u32,
) -> ConversationObserveCursor {
    let (incarnation, boundary, creation) = match base {
        Some(base) => {
            let creation = match base.creation.parse::<u64>() {
                Ok(value) => value.checked_add(1).map(|next| next.to_string()),
                Err(_) => None,
            };
            (
                base.incarnation.clone(),
                base.boundary.clone(),
                creation.unwrap_or_else(|| base.creation.clone()),
            )
        }
        None => ("shared".to_owned(), "0".to_owned(), "1".to_owned()),
    };
    ConversationObserveCursor {
        incarnation,
        boundary,
        creation,
        id: format!("{fingerprint:08x}-ffff-ffff-ffff-{offset:012x}"),
    }
}

/// A stable fingerprint of replica ids in order. A later page trusts its
/// offset only when the list still hashes to the cursor's fingerprint.
pub(super) fn replica_fingerprint(rows: &[ConversationSummary]) -> u32 {
    let mut hash = 0x811c_9dc5u32;
    for row in rows {
        for byte in row.conversation_id.bytes() {
            hash ^= u32::from(byte);
            hash = hash.wrapping_mul(0x0100_0193);
        }
        hash ^= 0xff;
    }
    hash
}

/// Drop replicas whose id is already a local conversation. Opening prefers
/// the local row, including one that is archived or deleted, so the list
/// must not show the peer's copy of it.
pub(super) fn omit_local(
    rows: Vec<ConversationSummary>,
    local: &HashSet<String>,
) -> Vec<ConversationSummary> {
    rows.into_iter()
        .filter(|row| !local.contains(&row.conversation_id))
        .collect()
}

/// The replica ids that are already local conversations, archived and deleted
/// included. `load` returns a tombstone, which is the same fact `open` uses
/// to prefer the local row.
pub(super) async fn local_ids(
    state: &ProductRouteState,
    rows: &[ConversationSummary],
) -> Result<HashSet<String>, ConversationError> {
    let mut local = HashSet::new();
    for row in rows {
        let Ok(id) = ConversationId::new(&row.conversation_id) else {
            continue;
        };
        if local_conversation(state, &id).await? {
            local.insert(row.conversation_id.clone());
        }
    }
    Ok(local)
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
) -> Result<SharedCatalogue, ConversationError> {
    let mut found = Vec::new();
    let mut complete = true;
    for peer in peers {
        match peer_rows(records, peer, &wall, &monotonic) {
            Ok((rows, rows_complete)) => {
                if !rows_complete {
                    complete = false;
                }
                found.push(rows);
            }
            Err(error) => {
                tracing::warn!(
                    receiver = %peer.receiver,
                    error = ?error,
                    "a peer cache was left out of the conversation list"
                );
                complete = false;
            }
        }
    }
    Ok(SharedCatalogue {
        rows: choose_peer_rows(found, archived),
        complete,
    })
}

/// The first peer to hold an id is the conversation every later read opens.
/// Its archived flag decides the list. A later peer's copy of that id is not
/// a second chat, and it does not replace an archived copy with an active one.
pub(super) fn choose_peer_rows(
    peers: Vec<Vec<ConversationSummary>>,
    archived: bool,
) -> Vec<ConversationSummary> {
    let mut seen = HashSet::new();
    let mut rows = Vec::new();
    for peer in peers {
        for row in peer {
            if !seen.insert(row.conversation_id.clone()) {
                continue;
            }
            if row.archived == archived {
                rows.push(row);
            }
        }
    }
    rows
}

fn find_earlier(
    records: &PeerRecords,
    peers: &[ActivePeer],
    wall: Arc<dyn WallClock>,
    monotonic: Arc<dyn MonotonicClock>,
    id: &ConversationId,
    before: &ConversationPoint,
    after: Option<&ConversationPoint>,
) -> Result<Option<ConversationEarlier>, ConversationError> {
    for peer in peers {
        match peer_earlier(records, peer, &wall, &monotonic, id, before, after) {
            Ok(EarlierHit::Elsewhere) => {}
            Ok(EarlierHit::Page(page)) => return Ok(Some(page)),
            Ok(EarlierHit::NotLoaded) => return Err(ConversationError::Unavailable),
            Ok(EarlierHit::NotInHistory) => return Err(ConversationError::InvalidInput),
            // This peer holds the id. A later peer's history is not a substitute.
            Err(error) => {
                tracing::warn!(
                    receiver = %peer.receiver,
                    error = ?error,
                    "a peer cache could not page earlier history"
                );
                return Err(ConversationError::Unavailable);
            }
        }
    }
    Ok(None)
}

enum EarlierHit {
    Elsewhere,
    /// The chat is retained, and its transcript is not loaded yet.
    NotLoaded,
    NotInHistory,
    Page(ConversationEarlier),
}

fn peer_earlier(
    records: &PeerRecords,
    peer: &ActivePeer,
    wall: &Arc<dyn WallClock>,
    monotonic: &Arc<dyn MonotonicClock>,
    id: &ConversationId,
    before: &ConversationPoint,
    after: Option<&ConversationPoint>,
) -> Result<EarlierHit, ReadFailure> {
    let mut cache = open_cache(records, peer, wall, monotonic)?;
    let held = cache.conversation_ids(&peer.receiver)?;
    if !held.iter().any(|held| held == &id.to_string()) {
        return Ok(EarlierHit::Elsewhere);
    }
    let Some((origin, _)) = held_place(&mut cache, &peer.receiver, id)? else {
        return Ok(EarlierHit::NotLoaded);
    };
    match cache.earlier(&peer.receiver, &origin, id, before, after) {
        Ok(Some(page)) => Ok(EarlierHit::Page(page)),
        Ok(None) => Ok(EarlierHit::NotLoaded),
        Err(EarlierMiss::Point) => Ok(EarlierHit::NotInHistory),
        Err(EarlierMiss::Read(error)) => Err(error),
    }
}

fn read_cached_file(
    records: &Arc<PeerRecords>,
    peers: &[ActivePeer],
    wall: Arc<dyn WallClock>,
    monotonic: Arc<dyn MonotonicClock>,
    id: &ConversationId,
    file: &SharedFileRequest,
) -> Result<SharedFileRead, ConversationError> {
    let mut failed = false;
    for peer in peers {
        let holds = match cache_holds(records, peer, &wall, &monotonic, id) {
            Ok(holds) => holds,
            Err(error) => {
                tracing::warn!(
                    receiver = %peer.receiver,
                    error = ?error,
                    "a peer cache could not be read for a shared file"
                );
                failed = true;
                continue;
            }
        };
        if !holds {
            continue;
        }
        let record = records
            .get(&peer.key)
            .map_err(|_| ConversationError::Unavailable)?;
        let Some(PeerEntry::Readable(record)) = record else {
            return Ok(SharedFileRead::Unavailable);
        };
        return Ok(shared_range(
            records,
            &record,
            &peer.receiver,
            id,
            file,
            &monotonic,
        ));
    }
    if failed {
        Err(ConversationError::Unavailable)
    } else {
        Ok(SharedFileRead::Elsewhere)
    }
}

fn cache_holds(
    records: &PeerRecords,
    peer: &ActivePeer,
    wall: &Arc<dyn WallClock>,
    monotonic: &Arc<dyn MonotonicClock>,
    id: &ConversationId,
) -> Result<bool, ReadFailure> {
    let mut cache = open_cache(records, peer, wall, monotonic)?;
    let held = cache.conversation_ids(&peer.receiver)?;
    Ok(held.iter().any(|held| held == &id.to_string()))
}

fn shared_range(
    records: &Arc<PeerRecords>,
    record: &PeerRecord,
    receiver: &str,
    id: &ConversationId,
    file: &SharedFileRequest,
    clock: &Arc<dyn MonotonicClock>,
) -> SharedFileRead {
    let read = read_granted_artifact(
        records,
        record,
        receiver,
        PeerArtifact {
            conversation: id,
            digest: &file.digest,
            media_type: &file.media_type,
            size: file.size,
            offset: file.offset,
            max_bytes: file.max_bytes,
        },
        Arc::clone(clock),
    );
    match read {
        Ok(bytes) => SharedFileRead::Range(bytes),
        Err(ReadFailure::Product(ConversationErrorCode::InvalidRequest)) => {
            SharedFileRead::Unavailable
        }
        Err(ReadFailure::Product(
            ConversationErrorCode::ConversationNotFound
            | ConversationErrorCode::ConversationDeleted,
        )) => SharedFileRead::Missing,
        Err(ReadFailure::Product(_)) => SharedFileRead::NotHeld,
        Err(_) => SharedFileRead::Unavailable,
    }
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
            Ok(PeerCopy::Opened(opened)) => return Ok(Some(*opened)),
            Ok(PeerCopy::Absent) => {}
            // The first peer that holds the id wins, including when its
            // transcript cannot be read. A later peer's copy is not this chat.
            Ok(PeerCopy::Unreadable) => return Err(ConversationError::Unavailable),
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

/// What one peer's cache holds for an id.
enum PeerCopy {
    /// This peer does not have the conversation.
    Absent,
    Opened(Box<OpenedReplica>),
    /// This peer has the conversation, and its transcript cannot be read.
    Unreadable,
}

/// Where a held chat's transcript was stored: the catalogue's origin and
/// stream, or the follow's copy of this one conversation.
fn held_place(
    cache: &mut RetainedCache,
    receiver: &str,
    id: &ConversationId,
) -> Result<Option<(String, String)>, ReadFailure> {
    let catalogue = cache.saved_catalogue_scope(receiver)?;
    let name = id.to_string();
    // A follow can copy a transcript after the catalogue snapshot. That copy's
    // origin is the one the transcript was stored under, which is not the
    // catalogue's when the catalogue was read first.
    let origin = match cache.transcript_origin(receiver, &name)? {
        Some(origin) => origin,
        None => match catalogue.as_ref() {
            Some(scope) => scope.origin().as_str().to_owned(),
            None => return Ok(None),
        },
    };
    let stream = catalogue
        .as_ref()
        .map(|scope| scope.stream().as_str().to_owned())
        .unwrap_or(name);
    Ok(Some((origin, stream)))
}

/// List rows for chats a follow copied without a catalogue.
///
/// The boolean is false when a retained transcript could not be read. Those
/// rows are left out, and the list stays incomplete, so the omission is not
/// a claim that the chat is gone.
fn grant_rows(
    cache: &mut RetainedCache,
    receiver: &str,
) -> Result<(Vec<ConversationSummary>, bool), ReadFailure> {
    let mut rows = Vec::new();
    let mut complete = true;
    for id in cache.conversation_ids(receiver)? {
        let Ok(conversation) = ConversationId::new(&id) else {
            continue;
        };
        let Some(origin) = cache.transcript_origin(receiver, &id)? else {
            continue;
        };
        let Some(transcript) = listed_transcript(
            cache.transcript_view(receiver, &origin, &conversation, Uuid::nil()),
            &mut complete,
        ) else {
            continue;
        };
        let RetainedTranscript::Retained(snapshot) = transcript else {
            continue;
        };
        let view = snapshot.view();
        // The fold's saved activity, not the clock of this read. An idle
        // chat stays where that time puts it.
        rows.push(ConversationSummary {
            conversation_id: view.conversation_id.clone(),
            title: view.title.clone(),
            preview: None,
            created_at_ms: snapshot.created_at_ms(),
            updated_at_ms: snapshot.updated_at_ms(),
            running: false,
            archived: false,
            channel_id: None,
            ask: None,
        });
    }
    Ok((rows, complete))
}

/// A transcript the cache could not read is not proof the chat is gone.
/// `complete` becomes false so the list does not treat the omission as a
/// withdrawal. A transcript the cache did answer, including one not loaded
/// yet, leaves `complete` as it was.
fn listed_transcript(
    transcript: Result<RetainedTranscript, ReadFailure>,
    complete: &mut bool,
) -> Option<RetainedTranscript> {
    match transcript {
        Ok(transcript) => Some(transcript),
        Err(error) => {
            tracing::warn!(
                error = ?error,
                "a retained transcript was left out of the conversation list"
            );
            *complete = false;
            None
        }
    }
}

fn peer_rows(
    records: &PeerRecords,
    peer: &ActivePeer,
    wall: &Arc<dyn WallClock>,
    monotonic: &Arc<dyn MonotonicClock>,
) -> Result<(Vec<ConversationSummary>, bool), ReadFailure> {
    let mut cache = open_cache(records, peer, wall, monotonic)?;
    let Some(scope) = cache.saved_catalogue_scope(&peer.receiver)? else {
        return grant_rows(&mut cache, &peer.receiver);
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
            if let Some(row) = summary_of(entry) {
                rows.push(row);
            }
        }
        let Some(next) = page.next().cloned() else {
            break;
        };
        after = Some(next);
    }
    // The catalogue snapshot stays as it was when the follow opened. A grant
    // revoked after that is gone from the replica and is not a row, even
    // though the snapshot still names it.
    let granted = cache.saved_grants(&peer.receiver)?;
    rows = rows_still_granted(rows, granted.as_deref());
    // A grant copied after this catalogue snapshot is not one of its entries.
    // A transcript that could not be read leaves the list incomplete rather
    // than reporting a complete list that does not name the chat.
    let (retained, retained_complete) = grant_rows(&mut cache, &peer.receiver)?;
    Ok((include_retained(rows, retained), retained_complete))
}

/// Catalogue rows, kept only when `granted` is absent or names them. A saved
/// grant frame is that membership: a row the snapshot still has after the
/// grant was revoked is not listed.
fn rows_still_granted(
    rows: Vec<ConversationSummary>,
    granted: Option<&[String]>,
) -> Vec<ConversationSummary> {
    let Some(granted) = granted else {
        return rows;
    };
    rows.into_iter()
        .filter(|row| granted.iter().any(|id| id == &row.conversation_id))
        .collect()
}

/// Catalogue rows, plus retained grants the snapshot does not name. A grant
/// the catalogue already lists stays that row. Its update time stays the
/// catalogue's unless the retained transcript's saved activity is later.
fn include_retained(
    mut rows: Vec<ConversationSummary>,
    retained: Vec<ConversationSummary>,
) -> Vec<ConversationSummary> {
    let mut position = std::collections::HashMap::new();
    for (index, row) in rows.iter().enumerate() {
        position.insert(row.conversation_id.clone(), index);
    }
    for row in retained {
        if let Some(index) = position.get(&row.conversation_id).copied() {
            if row.updated_at_ms > rows[index].updated_at_ms {
                rows[index].updated_at_ms = row.updated_at_ms;
            }
            continue;
        }
        position.insert(row.conversation_id.clone(), rows.len());
        rows.push(row);
    }
    rows
}

fn summary_of(entry: &RetainedCatalogueEntry) -> Option<ConversationSummary> {
    if entry.manifest().deleted {
        return None;
    }
    let metadata = entry.metadata()?;
    let summary = metadata.summary();
    let is_archived = summary.is_some_and(|summary| summary.archived());
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
        ask: None,
    })
}

fn peer_open(
    records: &PeerRecords,
    peer: &ActivePeer,
    wall: &Arc<dyn WallClock>,
    monotonic: &Arc<dyn MonotonicClock>,
    id: &ConversationId,
    project: bool,
) -> Result<PeerCopy, ReadFailure> {
    let mut cache = open_cache(records, peer, wall, monotonic)?;
    let held = cache.conversation_ids(&peer.receiver)?;
    let name = id.to_string();
    if !held.iter().any(|held| held == &name) {
        return Ok(PeerCopy::Absent);
    }
    if !project {
        return Ok(PeerCopy::Opened(Box::new(placeholder(id))));
    }
    // A catalogue read names its origin and stream. A follow stores each
    // transcript under the conversation's own stream and writes no catalogue.
    let Some((origin, stream)) = held_place(&mut cache, &peer.receiver, id)? else {
        return Ok(PeerCopy::Absent);
    };
    let selection = selection_for(&mut cache, &peer.receiver, &origin, &stream, id)?;
    let transcript = match cache.transcript_view(&peer.receiver, &origin, id, Uuid::nil()) {
        Ok(transcript) => transcript,
        Err(error) => {
            tracing::warn!(
                receiver = %peer.receiver,
                error = ?error,
                "a peer cache holds the chat and its transcript could not be read"
            );
            return Ok(PeerCopy::Unreadable);
        }
    };
    if matches!(
        transcript,
        RetainedTranscript::NotLoaded | RetainedTranscript::Deleted
    ) {
        // Not loaded yet: there is no transcript cursor. An empty incarnation
        // is not a view the client accepts, so the read stays unavailable
        // until the copy has one.
        return Ok(PeerCopy::Unreadable);
    }
    let opened = project_transcript(id, String::new(), selection, transcript);
    if opened.cursor.incarnation.is_empty() {
        return Ok(PeerCopy::Unreadable);
    }
    Ok(PeerCopy::Opened(Box::new(opened)))
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
            // The catalogue snapshot names the agent the list shows. It wins
            // over the provider the transcript fold recorded. A grant the
            // snapshot does not name keeps that provider.
            if let Some(selection) = selection {
                view.selection = Some(selection);
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

    use super::include_retained;

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
        ConversationCapabilities, ConversationEarlier, ConversationPoint,
        ConversationSelectionView, ConversationTranscriptState, ConversationView,
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

    fn summary(id: &str) -> ConversationSummary {
        ConversationSummary {
            conversation_id: id.to_owned(),
            title: Some(id.to_owned()),
            preview: None,
            created_at_ms: 1,
            updated_at_ms: 1,
            running: false,
            archived: false,
            channel_id: None,
            ask: None,
        }
    }

    #[test]
    fn a_grant_the_catalogue_snapshot_missed_is_listed_once() {
        let catalogue = vec![summary("already")];
        let mut copy = summary("already");
        copy.updated_at_ms = 50;
        let retained = vec![copy, summary("later")];
        let listed = include_retained(catalogue, retained);
        let ids: Vec<_> = listed
            .iter()
            .map(|row| row.conversation_id.as_str())
            .collect();
        assert_eq!(ids, ["already", "later"]);
        assert_eq!(
            listed[0].updated_at_ms, 50,
            "the copy is newer than the snapshot"
        );
        let mut older = summary("already");
        older.updated_at_ms = 80;
        let kept = include_retained(vec![older], vec![summary("already")]);
        assert_eq!(
            kept[0].updated_at_ms, 80,
            "an older copy leaves the catalogue time"
        );
    }

    #[test]
    fn a_shared_membership_longer_than_the_field_is_left_off() {
        let fitting: Vec<_> = (0..super::MAX_CATALOGUE_ENTRIES)
            .map(|n| format!("id-{n}"))
            .collect();
        assert_eq!(
            super::shared_membership(fitting.clone()).as_deref(),
            Some(fitting.as_slice())
        );
        let mut over = fitting;
        over.push("id-one-more".to_owned());
        assert!(super::shared_membership(over).is_none());
    }

    #[test]
    fn a_revoked_grant_drops_the_catalogue_row_the_snapshot_still_has() {
        let catalogue = vec![summary("revoked"), summary("kept")];
        let listed = super::rows_still_granted(catalogue, Some(&["kept".to_owned()]));
        let ids: Vec<_> = listed
            .iter()
            .map(|row| row.conversation_id.as_str())
            .collect();
        assert_eq!(ids, ["kept"]);
        let before = super::rows_still_granted(vec![summary("revoked")], None);
        assert_eq!(before.len(), 1);
    }

    #[test]
    fn a_transcript_that_cannot_be_read_leaves_the_list_incomplete() {
        let mut complete = true;
        assert!(super::listed_transcript(
            Err(nessa_client_core::retained::ReadFailure::Cache),
            &mut complete,
        )
        .is_none());
        assert!(!complete);
        let mut complete = true;
        assert!(matches!(
            super::listed_transcript(
                Ok(nessa_client_core::retained::RetainedTranscript::NotLoaded),
                &mut complete,
            ),
            Some(nessa_client_core::retained::RetainedTranscript::NotLoaded)
        ));
        assert!(complete);
    }

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
        ) -> Result<super::SharedCatalogue, crate::conversation::application::ConversationError>
        {
            if archived {
                return Ok(super::SharedCatalogue {
                    rows: Vec::new(),
                    complete: true,
                });
            }
            Ok(super::SharedCatalogue {
                complete: true,
                rows: vec![ConversationSummary {
                    conversation_id: self.id.clone(),
                    title: self.view.title.clone(),
                    preview: Some("hello from the grantor".into()),
                    created_at_ms: 10,
                    updated_at_ms: 11,
                    running: false,
                    archived: false,
                    channel_id: None,
                    ask: None,
                }],
            })
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

        async fn earlier(
            &self,
            id: &ConversationId,
            before: &ConversationPoint,
            _: Option<&ConversationPoint>,
        ) -> Result<Option<ConversationEarlier>, crate::conversation::application::ConversationError>
        {
            if id.to_string() != self.id {
                return Ok(None);
            }
            if before.execution_id != "kept-turn" {
                return Err(crate::conversation::application::ConversationError::InvalidInput);
            }
            Ok(Some(ConversationEarlier {
                conversation_id: self.id.clone(),
                messages: Vec::new(),
                tools: Vec::new(),
                handoffs: Vec::new(),
                more: false,
                truncated: false,
            }))
        }

        async fn read_file(
            &self,
            id: &ConversationId,
            file: super::SharedFileRequest,
        ) -> Result<super::SharedFileRead, crate::conversation::application::ConversationError>
        {
            if id.to_string() != self.id {
                return Ok(super::SharedFileRead::Elsewhere);
            }
            if file.digest == format!("sha256:{}", "ab".repeat(32))
                && file.offset == 0
                && file.size == 4
            {
                return Ok(super::SharedFileRead::Range(vec![1, 2, 3, 4]));
            }
            Ok(super::SharedFileRead::NotHeld)
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
        route(chats, ConversationLimits::default(), None).await
    }

    async fn route(
        chats: Arc<dyn SharedChats>,
        limits: ConversationLimits,
        listing: Option<Arc<dyn crate::conversation::application::ConversationListing>>,
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
        let service = match listing {
            Some(listing) => {
                crate::conversation_test_support::fixture_with_listing(limits, listing)
            }
            None => crate::conversation_test_support::fixture(limits).0,
        };
        let catalog = serde_json::from_value(json!({
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
    async fn a_replica_without_a_transcript_cursor_is_not_a_view() {
        let held = replica();
        let id = held.id.clone();
        // An empty incarnation is what a transcript that has not loaded used
        // to send. The client rejects it, so the read is unavailable.
        let empty = Arc::new(Held {
            id: id.clone(),
            view: held.view.clone(),
            cursor: CommittedCursor {
                incarnation: String::new(),
                position: 0,
                observed_head: 0,
                read_through: 0,
            },
        });
        let (state, session) = owner_state(empty).await;
        let read = ask(
            &state,
            &session,
            "conversation.read",
            json!({ "conversationId": id }),
        )
        .await;
        assert!(!read.ok, "{read:?}");
        assert_eq!(read.error.unwrap().code, "temporarily_unavailable");
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

        let earlier = ask(
            &state,
            &session,
            "conversation.readEarlier",
            json!({
                "conversationId": id,
                "before": { "executionId": "kept-turn" }
            }),
        )
        .await;
        assert!(earlier.ok, "{earlier:?}");
        let page = earlier.payload.unwrap();
        assert_eq!(page["conversationId"], id);
        assert_eq!(page["more"], false);
        let missing_point = ask(
            &state,
            &session,
            "conversation.readEarlier",
            json!({
                "conversationId": id,
                "before": { "executionId": "no-such-turn" }
            }),
        )
        .await;
        assert!(!missing_point.ok);
        assert_eq!(missing_point.error.unwrap().code, "invalid_request");

        let bare = serde_json::from_value(json!({
            "agents": [],
            "nativeHooks": {"claude": "off", "codex": "off"}
        }))
        .unwrap();
        let state = state.with_agents_catalog(bare);
        let read = ask(
            &state,
            &session,
            "conversation.read",
            json!({ "conversationId": id }),
        )
        .await;
        assert!(read.ok, "{read:?}");
        let payload = read.payload.unwrap();
        assert_eq!(payload["title"], "from the grantor");
        assert_eq!(payload["model"], "model");
        assert_eq!(payload["approvalMode"], "ask");
        assert!(payload.get("runtime").is_none());
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
        let earlier = ask(
            &state,
            &session,
            "conversation.readEarlier",
            json!({
                "conversationId": id,
                "before": { "executionId": "kept-turn" }
            }),
        )
        .await;
        assert!(!earlier.ok);
        assert_eq!(earlier.error.unwrap().code, "conversation_not_found");
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
            ask: None,
        };
        let mut rows: Vec<_> = (0..256)
            .map(|index| summary(&format!("local-{index}")))
            .collect();
        let mut complete = true;
        assert_eq!(
            super::append_bounded(&mut rows, vec![summary("shared")], 256, &mut complete, 0),
            Some(0)
        );
        assert_eq!(rows.len(), 256);
        assert!(!complete);
        let mut room = vec![summary("local")];
        complete = true;
        assert_eq!(
            super::append_bounded(&mut room, vec![summary("shared")], 256, &mut complete, 0),
            None
        );
        assert_eq!(room.len(), 2);
        assert!(complete);
        let mut continued = Vec::new();
        complete = true;
        assert_eq!(
            super::append_bounded(
                &mut continued,
                vec![summary("a"), summary("b"), summary("c")],
                1,
                &mut complete,
                1,
            ),
            Some(2)
        );
        assert_eq!(continued.len(), 1);
        assert_eq!(continued[0].conversation_id, "b");
        assert!(!complete);
        let mut same = Vec::new();
        complete = true;
        let mut first = summary("same");
        first.title = Some("first".into());
        let mut second = summary("same");
        second.title = Some("second".into());
        assert_eq!(
            super::append_bounded(
                &mut same,
                vec![first, second, summary("other")],
                256,
                &mut complete,
                0,
            ),
            None
        );
        assert_eq!(same.len(), 2);
        assert_eq!(same[0].title.as_deref(), Some("first"));
        assert_eq!(same[1].conversation_id, "other");
        assert!(complete);
        let mut partial = vec![summary("local")];
        complete = true;
        assert!(super::append_catalogue(
            &mut partial,
            super::SharedCatalogue {
                rows: vec![summary("shared")],
                complete: false,
            },
            256,
            &mut complete,
            0,
            None,
        )
        .next
        .is_none());
        assert_eq!(partial.len(), 2);
        assert!(!complete);
        let kept = vec![summary("kept"), summary("later")];
        let fingerprint = super::replica_fingerprint(&kept);
        let mut changed = Vec::new();
        complete = true;
        let stopped = super::append_catalogue(
            &mut changed,
            super::SharedCatalogue {
                rows: kept,
                complete: true,
            },
            1,
            &mut complete,
            1,
            Some(0),
        );
        assert!(changed.is_empty());
        assert!(stopped.next.is_none());
        assert!(!complete);
        assert_ne!(stopped.fingerprint, 0);
        let mut followed = Vec::new();
        complete = true;
        let rest = super::append_catalogue(
            &mut followed,
            super::SharedCatalogue {
                rows: vec![summary("kept"), summary("later")],
                complete: true,
            },
            1,
            &mut complete,
            1,
            Some(fingerprint),
        );
        assert_eq!(followed[0].conversation_id, "later");
        assert!(rest.next.is_none());
        let local = std::collections::HashSet::from([String::from("local")]);
        let replicas = super::omit_local(vec![summary("local"), summary("shared")], &local);
        assert_eq!(replicas.len(), 1);
        assert_eq!(replicas[0].conversation_id, "shared");
    }

    #[test]
    fn the_first_peer_wins_before_the_archived_filter() {
        let row = |id: &str, archived: bool| ConversationSummary {
            conversation_id: id.to_owned(),
            title: Some(if archived { "old" } else { "new" }.into()),
            preview: None,
            created_at_ms: 1,
            updated_at_ms: 1,
            running: false,
            archived,
            channel_id: None,
            ask: None,
        };
        let peers = || {
            vec![
                vec![row("same", true), row("only-first", false)],
                vec![row("same", false)],
            ]
        };
        let active = super::choose_peer_rows(peers(), false);
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].conversation_id, "only-first");
        let archived = super::choose_peer_rows(peers(), true);
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].title.as_deref(), Some("old"));
    }

    #[test]
    fn a_shared_page_cursor_is_later_than_the_local_one_and_names_its_offset() {
        use nessa_protocol::product::generated::ConversationObserveCursor;
        let local = ConversationObserveCursor {
            incarnation: "metadata".into(),
            boundary: "4".into(),
            creation: "9".into(),
            id: "00000000-0000-4000-8000-000000000001".into(),
        };
        let next = super::shared_page_cursor(Some(&local), 0, 0xab);
        assert_eq!(next.incarnation, local.incarnation);
        assert_eq!(next.boundary, local.boundary);
        assert_eq!(next.creation, "10");
        let mark = super::shared_page_mark(&next).unwrap();
        assert_eq!(mark.offset, 0);
        assert_eq!(mark.fingerprint, 0xab);
        let later = super::shared_page_cursor(Some(&next), 256, 0xab);
        assert_eq!(later.creation, "11");
        assert_eq!(super::shared_page_mark(&later).unwrap().offset, 256);
        assert!(later.id > next.id);
        let fresh = super::shared_page_cursor(None, 0, 1);
        assert_eq!(fresh.incarnation, "shared");
        assert_eq!(fresh.creation, "1");
        assert_eq!(super::shared_page_mark(&fresh).unwrap().offset, 0);
        assert!(super::shared_page_mark(&local).is_none());
        assert!(ConversationId::new(&next.id).is_err());
        assert!(ConversationId::new(&local.id).is_ok());
        let near = ConversationObserveCursor {
            id: "ffffffff-0000-4000-8000-000000000000".into(),
            ..local
        };
        assert!(ConversationId::new(&near.id).is_ok());
        assert!(super::shared_page_mark(&near).is_none());
    }

    /// The local catalogue's last page: full, finished, and without a cursor.
    /// The shared chat has to come back on a later page the desktop can ask for.
    struct FullPage;

    impl crate::conversation::application::ConversationListing for FullPage {
        fn list(
            &self,
            _: &OrganizationId,
            _: &nessa_auth::domain::PrincipalId,
            _: bool,
            _: usize,
            _: &crate::conversation::application::Reader,
        ) -> crate::conversation::application::ConversationFuture<
            '_,
            crate::conversation::application::ListedConversations,
        > {
            Box::pin(async { Ok(crate::conversation::application::ListedConversations::default()) })
        }

        fn observe(
            &self,
            _: &OrganizationId,
            _: &nessa_auth::domain::PrincipalId,
            _: bool,
            _: Option<crate::conversation::application::ObservationCursor>,
            _: &crate::conversation::application::Reader,
        ) -> crate::conversation::application::ConversationFuture<
            '_,
            crate::conversation::application::ObservedConversations,
        > {
            let conversations = (0..super::MAX_CATALOGUE_ENTRIES)
                .map(|index| {
                    let id = ConversationId::new(&format!("00000000-0000-4000-8000-{index:012x}"))
                        .unwrap();
                    crate::conversation::application::ListedConversation {
                        conversation: crate::conversation::domain::Conversation::new(
                            id,
                            OrganizationId::new("org").unwrap(),
                            nessa_auth::domain::PrincipalId::new("owner").unwrap(),
                            "credential".into(),
                            "create".into(),
                            1,
                            nessa_protocol::agents::AgentId::Claude,
                            nessa_protocol::conversation::domain::ConversationModelId::new("model")
                                .unwrap(),
                            nessa_protocol::conversation::domain::ConversationApprovalMode::Ask,
                        )
                        .unwrap(),
                        summary: nessa_protocol::conversation::domain::ConversationSummary::new(
                            None, None, 1, false,
                        )
                        .unwrap(),
                        held: None,
                    }
                })
                .collect();
            Box::pin(async move {
                Ok(crate::conversation::application::ObservedConversations {
                    conversations,
                    complete: true,
                    cursor: None,
                })
            })
        }

        fn owners(
            &self,
        ) -> crate::conversation::application::ConversationFuture<
            '_,
            Vec<(OrganizationId, nessa_auth::domain::PrincipalId)>,
        > {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    #[tokio::test]
    async fn a_full_observe_page_continues_into_the_shared_chat() {
        let held = replica();
        let shared_id = held.id.clone();
        let (state, session) = route(
            held,
            ConversationLimits::default(),
            Some(Arc::new(FullPage)),
        )
        .await;
        let first = ask(&state, &session, "conversation.observe", json!({})).await;
        assert!(first.ok, "{first:?}");
        let page = first.payload.unwrap();
        assert_eq!(page["complete"], false);
        let rows = page["conversations"].as_array().unwrap();
        assert_eq!(rows.len(), super::MAX_CATALOGUE_ENTRIES);
        assert!(rows.iter().all(|row| row["conversationId"] != shared_id));
        assert!(page["cursor"].is_object(), "{page}");
        let next = ask(
            &state,
            &session,
            "conversation.observe",
            json!({ "cursor": page["cursor"] }),
        )
        .await;
        assert!(next.ok, "{next:?}");
        let rest = next.payload.unwrap();
        assert_eq!(rest["complete"], true);
        assert!(rest.get("cursor").is_none());
        let shared = rest["conversations"].as_array().unwrap();
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0]["conversationId"], shared_id);

        // A later local page: the cursor has to stay in that pass and move
        // forward, which is the only continuation the desktop follows.
        let later = ask(
            &state,
            &session,
            "conversation.observe",
            json!({
                "cursor": {
                    "incarnation": "metadata",
                    "boundary": "4",
                    "creation": "9",
                    "id": "00000000-0000-4000-8000-000000000001"
                }
            }),
        )
        .await;
        assert!(later.ok, "{later:?}");
        let later = later.payload.unwrap();
        assert_eq!(later["complete"], false);
        assert_eq!(later["cursor"]["incarnation"], "metadata");
        assert_eq!(later["cursor"]["boundary"], "4");
        assert_eq!(later["cursor"]["creation"], "10");
        let followed = ask(
            &state,
            &session,
            "conversation.observe",
            json!({ "cursor": later["cursor"] }),
        )
        .await;
        assert!(followed.ok, "{followed:?}");
        let followed = followed.payload.unwrap();
        assert_eq!(followed["complete"], true);
        assert!(followed.get("cursor").is_none());
        assert_eq!(followed["conversations"][0]["conversationId"], shared_id);
    }

    #[tokio::test]
    async fn a_shared_chat_file_is_read_from_the_peer() {
        let held = replica();
        let id = held.id.clone();
        let (state, session) = owner_state(held).await;
        let digest = format!("sha256:{}", "ab".repeat(32));
        let read = |conversation: &str, digest: &str| {
            RequestFrame::new(
                "request",
                "artifact.read",
                &json!({
                    "conversationId": conversation,
                    "digest": digest,
                    "mimeType": "text/plain",
                    "size": 4,
                    "offset": 0,
                    "maxBytes": 4
                }),
            )
            .unwrap()
        };
        let reply = response(
            super::super::attachment::dispatch_read(&state, &session, read(&id, &digest)).await,
        );
        assert!(reply.ok, "{reply:?}");
        assert_eq!(reply.payload.unwrap()["data"], "AQIDBA==");
        let missing = response(
            super::super::attachment::dispatch_read(
                &state,
                &session,
                read("00000000-0000-4000-8000-000000000891", &digest),
            )
            .await,
        );
        assert!(!missing.ok);
        let other = response(
            super::super::attachment::dispatch_read(
                &state,
                &session,
                read(&id, &format!("sha256:{}", "cd".repeat(32))),
            )
            .await,
        );
        assert!(!other.ok);
        assert_eq!(other.error.unwrap().code, "attachment_not_found");
    }
}
