//! One subscription's task (`docs/design/record-subscriptions.md`).
//!
//! It registers every source a change can come from, then reads, sends what
//! changed, waits for the writer to write it, and reads again when a source
//! says something changed — or at once while the transcript is still being
//! replayed in bounded reads. Registration comes before the first read, so a
//! change that lands during or after any read leaves a source dirty, and the
//! next wait returns at once: no change is missed between replay and live
//! (row S7).
//!
//! Every batch is admitted by [`authorize_batch`] and nothing else.

use super::delivery::{Deadline, Outgoing, SubscriptionDeliveries};
use crate::conversation::application::{
    error_code, CatalogueChangeWatch, CatalogueWatchError, CatalogueWatchState, ConversationError,
    ConversationService, ReadOpening,
};
use crate::product::{
    conversation::{caller, read_list, read_view},
    passive_read::deadlines::{PASSIVE_READ_TIMEOUT, RECORD_SEND_TIMEOUT},
    socket::{admit_now, failure, success},
    state::{note_limit, ProductRouteState},
};
use nessa_auth::application::session::AuthenticatedSession;
use nessa_protocol::conversation::{domain::ConversationId, projection::CommittedCursor};
use nessa_protocol::product::generated::{
    product_event, product_method, ConversationListResult, ConversationListed,
    ConversationSubscribeResult, ConversationSubscriptionEndReason, ConversationSubscriptionEnded,
    ConversationSubscriptionErrorCode, ConversationView, ConversationViewCursor,
    ConversationViewed, MAX_PAYLOAD_BYTES, SUBSCRIPTION_DELIVERY_TIMEOUT_MS,
};
use nessa_protocol::product::passive_read::decimal_u64;
use nessa_protocol::protocol::{EventFrame, OutgoingMessage};
use nessa_sdk::application::agent_execution::sessions::{
    ChangeWatchError, ChangeWatchState, CommittedChangeWatch, StorageError,
};
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::{watch, OwnedSemaphorePermit},
    time::{timeout_at, Instant},
};

/// How long a frame may wait for the writer before its subscription is
/// ended as lagging (row S10).
pub(super) const DELIVERY_TIMEOUT: Duration =
    Duration::from_millis(SUBSCRIPTION_DELIVERY_TIMEOUT_MS);

/// Room an event's envelope takes beside its payload, measured on the
/// largest the writer frames: the longer of the two frame names, and the
/// largest sequence and state version. The frame bound the writer checks
/// (`ordinary_text` in `socket.rs`) is on the whole text, so a payload
/// `fits` admits is never refused there (row S22).
fn envelope_bytes() -> usize {
    static BYTES: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *BYTES.get_or_init(|| {
        [
            product_event::CONVERSATION_VIEW,
            product_event::CONVERSATION_LISTED,
        ]
        .into_iter()
        .map(|name| {
            let frame = EventFrame::push(name, &Value::Null, u64::MAX, u64::MAX)
                .expect("a JSON value serializes");
            OutgoingMessage::Event(frame)
                .to_wire_text()
                .expect("an event frame serializes")
                .len()
                - "null".len()
        })
        .max()
        .expect("two names")
    })
}

/// After a wake, a list waits this long before it reads, so a stream of
/// commits (a reply being saved every 100 ms) costs a few list reads a
/// second, not one per commit. Changes during the wait still collapse into
/// the next read. A view never waits (row L4).
pub(crate) const LIST_REREAD_FLOOR: Duration = Duration::from_millis(250);

/// Reads refused for want of a storage read slot are tried again after a
/// short wait, this many times, before the subscription is refused: one
/// commit wakes every subscription at once, and the SDK runs a few reads at
/// a time.
const TRANSIENT_RETRIES: u32 = 6;
const TRANSIENT_BACKOFF: Duration = Duration::from_millis(50);
const TRANSIENT_BACKOFF_CAP: Duration = Duration::from_millis(1000);

/// A view's place in the committed history, as the wire names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Cursor {
    pub incarnation: String,
    pub position: u64,
}

impl Cursor {
    pub fn decode(wire: &ConversationViewCursor) -> Option<Self> {
        Some(Self {
            incarnation: wire.incarnation.clone(),
            position: decimal_u64(&wire.position).ok()?,
        })
    }

    fn wire(&self) -> ConversationViewCursor {
        ConversationViewCursor {
            incarnation: self.incarnation.clone(),
            position: self.position.to_string(),
        }
    }

    /// Whether this is behind `floor` in the same history. Another
    /// incarnation is never behind: it replaces what the client holds.
    fn behind(&self, floor: &Self) -> bool {
        self.incarnation == floor.incarnation && self.position < floor.position
    }
}

/// What a subscription follows.
#[derive(Clone, PartialEq, Eq)]
pub(super) enum Target {
    View {
        conversation: ConversationId,
        after: Option<Cursor>,
    },
    List {
        archived: bool,
    },
}

impl Target {
    /// Two subscriptions to the same thing on one connection are one too many
    /// (row S18).
    pub fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::View { conversation, .. },
                Self::View {
                    conversation: other,
                    ..
                },
            ) => conversation == other,
            (Self::List { archived }, Self::List { archived: other }) => archived == other,
            _ => false,
        }
    }

    fn method(&self) -> &'static str {
        match self {
            Self::View { .. } => product_method::CONVERSATION_SUBSCRIBE,
            Self::List { .. } => product_method::CONVERSATION_SUBSCRIBE_LIST,
        }
    }
}

/// Everything a change can come from, registered before the first read.
struct Sources {
    records: CommittedChangeWatch,
    catalogue: Option<CatalogueChangeWatch>,
    live: Option<watch::Receiver<u64>>,
}

enum Wake {
    Changed,
    /// The owner's catalogue changed: a row the list frame left out may
    /// have changed too (row L5).
    Catalogue,
    Closed,
}

impl Sources {
    fn register(
        state: &ProductRouteState,
        service: &ConversationService,
        session: &AuthenticatedSession,
        target: &Target,
    ) -> Result<Self, String> {
        let unavailable = || ConversationError::Unavailable;
        let records = state
            .record_watches
            .as_ref()
            .ok_or_else(|| code_of(&unavailable()))?;
        let capacity = |error: ChangeWatchError| match error {
            ChangeWatchError::Capacity => ConversationSubscriptionErrorCode::SubscriptionCapacity
                .as_str()
                .to_owned(),
            ChangeWatchError::Closed => code_of(&unavailable()),
        };
        Ok(match target {
            Target::View { conversation, .. } => Self {
                records: records.watch(conversation).map_err(capacity)?,
                catalogue: None,
                // A view also shows what is live and not a record.
                live: Some(service.live_changes(conversation)),
            },
            Target::List { .. } => {
                let owner = caller(session, String::new());
                let catalogue = state
                    .catalogue_watches
                    .as_ref()
                    .ok_or_else(|| code_of(&unavailable()))?
                    .watch(&owner.organization_id, &owner.principal_id)
                    .map_err(|CatalogueWatchError::Capacity| {
                        ConversationSubscriptionErrorCode::SubscriptionCapacity
                            .as_str()
                            .to_owned()
                    })?;
                Self {
                    // A list row's `running` follows a turn's commits, which
                    // change no summary (row L2).
                    records: records.watch_any().map_err(capacity)?,
                    catalogue: Some(catalogue),
                    live: None,
                }
            }
        })
    }

    /// Takes every notice already waiting without waiting for another, and
    /// says whether one of them was the catalogue's. Called right before a
    /// list reads, so a commit that dirtied both the records and the
    /// catalogue is one read, not two (row L5). A notice after this is still
    /// waiting for the next `changed`; a closed source is said by it.
    async fn drain(&mut self) -> bool {
        let mut catalogue = false;
        while let Ok(wake) = tokio::time::timeout(Duration::ZERO, self.changed()).await {
            match wake {
                Wake::Changed => {}
                Wake::Catalogue => catalogue = true,
                Wake::Closed => break,
            }
        }
        catalogue
    }

    async fn changed(&mut self) -> Wake {
        let catalogue = async {
            match self.catalogue.as_mut() {
                Some(catalogue) => catalogue.changed().await,
                None => std::future::pending().await,
            }
        };
        let live = async {
            match self.live.as_mut() {
                Some(live) => live.changed().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            state = self.records.changed() => match state {
                ChangeWatchState::Dirty => Wake::Changed,
                ChangeWatchState::Closed => Wake::Closed,
            },
            state = catalogue => match state {
                CatalogueWatchState::Dirty => Wake::Catalogue,
                CatalogueWatchState::Closed | CatalogueWatchState::NotificationFailed => Wake::Closed,
            },
            changed = live => match changed {
                Ok(()) => Wake::Changed,
                Err(_) => Wake::Closed,
            },
        }
    }
}

/// The one admission of a subscription batch: the session is still current,
/// the method's grant still holds under current policy, and the browser
/// session is still present. Asked before every read, so access lost between
/// two batches ends the subscription before the next (row S14). Slice G's
/// per-conversation read grant is checked here and nowhere else.
pub(super) async fn authorize_batch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    target: &Target,
) -> Result<AuthenticatedSession, &'static str> {
    admit_now(state, session, target.method()).await
}

/// One read's result: a view and where it was folded through, or a list.
enum Batch {
    View {
        /// Boxed: a view is far larger than a list's result.
        view: Box<ConversationView>,
        cursor: Cursor,
        /// The history's head as that read observed it.
        head: u64,
    },
    List(ConversationListResult),
}

fn code_of(error: &ConversationError) -> String {
    error_code(error).as_str().to_owned()
}

/// How long to wait before reading again after `error` on attempt `attempt`
/// (from 0), or `None` to refuse the batch. Only a read refused for want of
/// a storage read slot is tried again, `TRANSIENT_RETRIES` times, the wait
/// doubling from `TRANSIENT_BACKOFF` up to `TRANSIENT_BACKOFF_CAP` (row S23).
fn retry_after(error: &ConversationError, attempt: u32) -> Option<Duration> {
    let transient = matches!(
        error,
        ConversationError::Storage(StorageError::ReadCapacity | StorageError::Busy)
    );
    if !transient || attempt >= TRANSIENT_RETRIES {
        return None;
    }
    Some((TRANSIENT_BACKOFF * 2u32.saturating_pow(attempt)).min(TRANSIENT_BACKOFF_CAP))
}

/// The subscription's first read, refused `unavailable` when it has not
/// finished by `read_by`, the passive read deadline of the subscribe request
/// (row S25): the reply waits on it, and leaves the rest of the request's
/// delivery budget for the reply's own write (row S31), as a record read does.
async fn by_reply<T>(
    read_by: Instant,
    read: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    timeout_at(read_by, read)
        .await
        .unwrap_or_else(|_| Err(code_of(&ConversationError::Unavailable)))
}

async fn read_batch(
    state: &ProductRouteState,
    service: &ConversationService,
    session: &AuthenticatedSession,
    target: &Target,
    request_id: &str,
) -> Result<Batch, String> {
    let mut attempt = 0;
    loop {
        // A batch is a request of the gateway's like any other: it holds its
        // capacity, then is admitted, then reads, as `dispatch` orders a
        // request. Admitted before waiting for capacity, a grant revoked
        // during the wait would let one read through (row S30).
        let permit = state.requests.clone().acquire_owned().await;
        let current = authorize_batch(state, session, target).await?;
        let read = match target {
            Target::View { conversation, .. } => {
                // A follower opens nothing (row S26).
                let read = read_view(
                    state,
                    service,
                    &current,
                    conversation,
                    request_id,
                    ReadOpening::LiveOnly,
                );
                match read.await {
                    Ok((view, Some(cursor))) => Ok(Batch::View {
                        view: Box::new(view),
                        head: cursor.observed_head,
                        cursor: cursor_of(cursor),
                    }),
                    Ok((_, None)) => Err(ConversationError::Unavailable),
                    Err(error) => Err(error),
                }
            }
            Target::List { archived } => read_list(service, &current, *archived, request_id)
                .await
                .map(Batch::List),
        };
        drop(permit);
        match read {
            Ok(batch) => return Ok(batch),
            Err(error) => match retry_after(&error, attempt) {
                Some(wait) => {
                    attempt += 1;
                    tokio::time::sleep(wait).await;
                }
                None => return Err(code_of(&error)),
            },
        }
    }
}

fn cursor_of(cursor: CommittedCursor) -> Cursor {
    Cursor {
        incarnation: cursor.incarnation,
        position: cursor.position,
    }
}

/// Everything one subscription task needs, handed over when it is spawned.
pub(super) struct Run {
    pub state: ProductRouteState,
    pub service: Arc<ConversationService>,
    pub session: AuthenticatedSession,
    pub id: String,
    pub request: String,
    pub target: Target,
    pub deliveries: Arc<SubscriptionDeliveries>,
    pub slot: Arc<OwnedSemaphorePermit>,
    /// The first read answers by here or the subscription is refused, so a
    /// first read that never returns cannot hold the request's slot past
    /// the passive read budget (row S25).
    pub read_by: Instant,
    /// The reply is written by here or the socket closes (row S31): within
    /// the passive delivery budget, which a client's call outlasts.
    pub reply_by: Instant,
}

impl Run {
    // One per thing a subscription task owns; a struct would only rename them.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        state: ProductRouteState,
        service: Arc<ConversationService>,
        session: AuthenticatedSession,
        id: String,
        request: String,
        target: Target,
        deliveries: Arc<SubscriptionDeliveries>,
        slot: Arc<OwnedSemaphorePermit>,
        received_at: Instant,
    ) -> Self {
        Self {
            state,
            service,
            session,
            id,
            request,
            target,
            deliveries,
            slot,
            read_by: received_at + PASSIVE_READ_TIMEOUT,
            reply_by: received_at + RECORD_SEND_TIMEOUT,
        }
    }
}

/// What this subscription last had written, and the floor no frame goes behind.
struct Sent {
    deliveries: Arc<SubscriptionDeliveries>,
    id: String,
    floor: Option<Cursor>,
    last: Option<(Option<Cursor>, Value)>,
    /// Where the last read of a view got to, sent or not.
    read_through: Option<Cursor>,
    /// How many catalogue changes a list has been woken by. Part of an
    /// incomplete list's key, with every row the service read, so a change
    /// to a row the frame left out is sent though the rows it carries are
    /// the same, and the client walks the catalogue for it (row L5).
    catalogue: u64,
}

impl Sent {
    /// Offer a frame and wait until it is written. False when it never will
    /// be: the subscription was ended as lagging (row S10), or the socket went.
    async fn offer(&mut self, payload: Value, cursor: Option<Cursor>, key: Value) -> bool {
        let written = self.deliveries.offer(
            &self.id,
            Outgoing::Event {
                name: match cursor {
                    Some(_) => product_event::CONVERSATION_VIEW,
                    None => product_event::CONVERSATION_LISTED,
                },
                payload,
            },
            Deadline::None,
            None,
        );
        tokio::pin!(written);
        let delivered = tokio::select! {
            result = &mut written => result.is_ok(),
            () = tokio::time::sleep(DELIVERY_TIMEOUT) => {
                if self.deliveries.withdraw(&self.id) {
                    note_limit("socket.subscription_delivery_deadline");
                    self.end(ConversationSubscriptionEndReason::Lagging, None);
                    return false;
                }
                // Taken just now: the socket's write timeout bounds it.
                (&mut written).await.is_ok()
            }
        };
        if delivered {
            if let Some(cursor) = &cursor {
                self.floor = Some(cursor.clone());
            }
            self.last = Some((cursor, key));
        }
        delivered
    }

    /// The subscription's one terminal frame. Written by its deadline or the
    /// socket closes, as a watch's terminal notice does.
    fn end(&self, reason: ConversationSubscriptionEndReason, code: Option<String>) {
        let payload = serde_json::to_value(ConversationSubscriptionEnded {
            subscription_id: self.id.clone(),
            reason,
            code,
            last_delivered: self.floor.as_ref().map(Cursor::wire),
        })
        .expect("generated payload serializes");
        drop(self.deliveries.offer(
            &self.id,
            Outgoing::Event {
                name: product_event::CONVERSATION_SUBSCRIPTION_ENDED,
                payload,
            },
            Deadline::Last(Instant::now() + DELIVERY_TIMEOUT),
            None,
        ));
    }

    /// Send `batch` if the client would see something new. Returns whether
    /// the subscription goes on, and whether the transcript is still being
    /// replayed.
    async fn deliver(&mut self, batch: Batch) -> Option<bool> {
        match self.frame(batch) {
            Framed::Behind { more } => Some(more),
            Framed::Frame(frame) => self.deliver_frame(frame).await,
        }
    }

    fn frame(&mut self, batch: Batch) -> Framed {
        match batch {
            Batch::View { view, cursor, .. } => {
                // A bounded read stopped short of the head it saw: read on at
                // once, while each read gets further. One that got nowhere
                // waits for a wake, so a head that cannot be reached now is
                // not read in a loop.
                let more =
                    view.transcript_state == "stale" && self.read_through.as_ref() != Some(&cursor);
                self.read_through = Some(cursor.clone());
                // Never a frame behind what the client has (row S5).
                if self
                    .floor
                    .as_ref()
                    .is_some_and(|floor| cursor.behind(floor))
                {
                    return Framed::Behind { more };
                }
                let payload = serde_json::to_value(ConversationViewed {
                    subscription_id: self.id.clone(),
                    cursor: cursor.wire(),
                    view: *view,
                })
                .expect("generated payload serializes");
                let key = view_key(&payload["view"]);
                Framed::Frame(Frame {
                    payload,
                    cursor: Some(cursor),
                    key,
                    more,
                })
            }
            Batch::List(list) => {
                // Every row the service read, not only those the frame
                // keeps: a row cut from the frame whose `running` changed by
                // a commit alone (no catalogue change) is still a change. A
                // complete list is the frame's own, uncut. One more
                // serialization of a list the service bounds, per read.
                let read = serde_json::to_value(&list).expect("generated list serializes");
                let payload = serde_json::to_value(ConversationListed {
                    subscription_id: self.id.clone(),
                    list: fitted(list, &self.id),
                })
                .expect("generated payload serializes");
                let key = if payload["list"]["complete"] == true {
                    read
                } else {
                    serde_json::json!([read, self.catalogue])
                };
                Framed::Frame(Frame {
                    payload,
                    cursor: None,
                    key,
                    more: false,
                })
            }
        }
    }

    async fn deliver_frame(&mut self, frame: Frame) -> Option<bool> {
        let Frame {
            payload,
            cursor,
            key,
            more,
        } = frame;
        if self
            .last
            .as_ref()
            .is_some_and(|(last_cursor, last_key)| *last_cursor == cursor && *last_key == key)
        {
            // Nothing the client would see changed (row S9).
            return Some(more);
        }
        if !fits(&payload) {
            self.end(ConversationSubscriptionEndReason::TooLarge, None);
            return None;
        }
        self.offer(payload, cursor, key).await.then_some(more)
    }
}

/// One read made into a frame, or a read behind what the client has.
enum Framed {
    Behind { more: bool },
    Frame(Frame),
}

struct Frame {
    payload: Value,
    cursor: Option<Cursor>,
    // What the client would see: the view or the list, without the
    // subscription id, compared with the last frame written.
    key: Value,
    more: bool,
}

/// What a client would see of a view: all of it but its revision, which a
/// fresh projection numbers anew though nothing in it changed (a
/// conversation read with no agent, row S26), so an equal view is not sent
/// again (row S9).
fn view_key(view: &Value) -> Value {
    let mut key = view.clone();
    if let Some(fields) = key.as_object_mut() {
        fields.remove("revision");
    }
    key
}

/// Whether an event with `payload` fits the socket's frame bound.
fn fits(payload: &Value) -> bool {
    serde_json::to_string(payload)
        .map(|text| text.len() + envelope_bytes() <= MAX_PAYLOAD_BYTES as usize)
        .unwrap_or(false)
}

/// The list cut, newest first, to what fits one frame, and marked incomplete
/// when anything was cut: the client walks `conversation.observe` for the
/// rest, as for any incomplete list (row L3). Compact JSON is the sum of its
/// parts, so each row is measured once.
fn fitted(mut list: ConversationListResult, id: &str) -> ConversationListResult {
    let empty = serde_json::to_string(&ConversationListed {
        subscription_id: id.to_owned(),
        list: ConversationListResult {
            conversations: Vec::new(),
            complete: false,
        },
    })
    .expect("generated payload serializes")
    .len();
    let budget = (MAX_PAYLOAD_BYTES as usize).saturating_sub(envelope_bytes());
    let mut size = empty;
    let mut keep = 0;
    for (index, row) in list.conversations.iter().enumerate() {
        let row = serde_json::to_string(row)
            .expect("generated row serializes")
            .len();
        // A comma before every row but the first.
        size += row + usize::from(index > 0);
        if size > budget {
            break;
        }
        keep += 1;
    }
    if keep < list.conversations.len() {
        list.conversations.truncate(keep);
        list.complete = false;
    }
    list
}

/// Run one subscription until it ends or its connection lets it go.
pub(super) async fn run(run: Run) {
    let Run {
        state,
        service,
        session,
        id,
        request,
        target,
        deliveries,
        slot,
        read_by,
        reply_by,
    } = run;
    let refuse = |code: &str, slot: Arc<OwnedSemaphorePermit>| {
        drop(deliveries.offer(
            &id,
            Outgoing::Reply(failure(&request, code)),
            Deadline::Last(Instant::now() + DELIVERY_TIMEOUT),
            Some(slot),
        ));
    };
    // Admitted before anything is registered: a session that may not follow
    // takes nothing from the shared watch pools (row S29).
    if let Err(code) = authorize_batch(&state, &session, &target).await {
        return refuse(code, slot);
    }
    let mut sources = match Sources::register(&state, &service, &session, &target) {
        Ok(sources) => sources,
        Err(code) => return refuse(&code, slot),
    };
    let after = match &target {
        Target::View { after, .. } => after.clone(),
        Target::List { .. } => None,
    };
    let mut sent = Sent {
        deliveries: deliveries.clone(),
        id: id.clone(),
        floor: after.clone(),
        last: None,
        read_through: None,
        catalogue: 0,
    };
    let mut reply = Some(slot);
    loop {
        let batch = if reply.is_some() {
            by_reply(
                read_by,
                read_batch(&state, &service, &session, &target, &request),
            )
            .await
        } else {
            read_batch(&state, &service, &session, &target, &request).await
        };
        let batch = match (batch, reply.take()) {
            (Ok(batch), None) => batch,
            (Ok(batch), Some(slot)) => {
                // A client ahead of the history the store holds (row S3).
                if let (Batch::View { cursor, head, .. }, Some(after)) = (&batch, &after) {
                    if after.incarnation == cursor.incarnation && after.position > *head {
                        return refuse(
                            ConversationSubscriptionErrorCode::CursorAhead.as_str(),
                            slot,
                        );
                    }
                }
                let answer = success(
                    &request,
                    &ConversationSubscribeResult {
                        subscription_id: id.clone(),
                    },
                );
                // The reply is this subscription's first frame, so it is
                // written before any event of it (row S1). It is owed by
                // the request's reply deadline, as a watch's reply is: one
                // written later than the client waits would leave it a live
                // subscription it cannot name, so missing it closes the
                // socket (row S31).
                if deliveries
                    .offer(
                        &id,
                        Outgoing::Reply(answer),
                        Deadline::Reply(reply_by),
                        Some(slot),
                    )
                    .await
                    .is_err()
                {
                    return;
                }
                batch
            }
            (Err(code), Some(slot)) => return refuse(&code, slot),
            (Err(code), None) => {
                sent.end(ConversationSubscriptionEndReason::Refused, Some(code));
                return;
            }
        };
        let Some(more) = sent.deliver(batch).await else {
            return;
        };
        if more {
            // Still replaying a long history: the next bounded read now.
            continue;
        }
        match sources.changed().await {
            Wake::Changed => {}
            Wake::Catalogue => sent.catalogue += 1,
            Wake::Closed => {
                sent.end(ConversationSubscriptionEndReason::SourceClosed, None);
                return;
            }
        }
        if matches!(target, Target::List { .. }) {
            tokio::time::sleep(LIST_REREAD_FLOOR).await;
            if sources.drain().await {
                sent.catalogue += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::product::event_sequence::EventSequence;
    use nessa_protocol::product::generated::ConversationSummary;

    fn row(n: usize, preview: usize) -> ConversationSummary {
        ConversationSummary {
            conversation_id: format!("00000000-0000-4000-8000-{n:012}"),
            title: Some("t".into()),
            preview: Some("p".repeat(preview)),
            created_at_ms: n as u64,
            updated_at_ms: n as u64,
            running: false,
            archived: false,
        }
    }

    /// Row L3.
    #[test]
    fn a_list_too_large_for_one_frame_is_cut_and_marked_incomplete() {
        let small = ConversationListResult {
            conversations: (0..3).map(|n| row(n, 10)).collect(),
            complete: true,
        };
        let kept = fitted(small, "1");
        assert!(kept.complete);
        assert_eq!(kept.conversations.len(), 3);
        let large = ConversationListResult {
            conversations: (0..500).map(|n| row(n, 400)).collect(),
            complete: true,
        };
        let cut = fitted(large, "1");
        assert!(!cut.complete);
        assert!(cut.conversations.len() < 500 && !cut.conversations.is_empty());
        assert_eq!(cut.conversations[0].created_at_ms, 0, "newest first kept");
        let payload = serde_json::to_value(ConversationListed {
            subscription_id: "1".into(),
            list: cut,
        })
        .unwrap();
        assert!(fits(&payload));
    }

    /// Row L5: a row cut from an incomplete frame whose `running` changed by
    /// a commit alone (no catalogue change) changes the frame's key, though
    /// the rows the frame carries are the same, so the frame is sent and the
    /// client walks the catalogue.
    #[test]
    fn a_commit_to_a_row_the_frame_cut_is_still_a_change() {
        let deliveries = Arc::new(SubscriptionDeliveries::new(Arc::new(
            EventSequence::default(),
        )));
        let mut sent = sent(&deliveries);
        let list = |running: bool| {
            let mut rows: Vec<_> = (0..500).map(|n| row(n, 400)).collect();
            rows[499].running = running;
            Batch::List(ConversationListResult {
                conversations: rows,
                complete: true,
            })
        };
        let key = |sent: &mut Sent, batch| match sent.frame(batch) {
            Framed::Frame(frame) => (frame.payload["list"].clone(), frame.key),
            Framed::Behind { .. } => panic!("a list is never behind"),
        };
        let (idle_rows, idle) = key(&mut sent, list(false));
        let (running_rows, running) = key(&mut sent, list(true));
        assert_eq!(idle_rows["complete"], false, "cut");
        assert_eq!(idle_rows, running_rows, "the frame carries the same rows");
        assert_ne!(idle, running);
    }

    fn sent(deliveries: &Arc<SubscriptionDeliveries>) -> Sent {
        Sent {
            deliveries: deliveries.clone(),
            id: "1".into(),
            floor: None,
            last: None,
            read_through: None,
            catalogue: 0,
        }
    }

    fn ended(deliveries: &SubscriptionDeliveries) -> Value {
        let frame = deliveries.take().expect("a terminal frame");
        assert!(frame.deadline.last());
        let OutgoingMessage::Event(event) = frame.message else {
            panic!("an event")
        };
        assert_eq!(event.event, product_event::CONVERSATION_SUBSCRIPTION_ENDED);
        event.payload
    }

    /// Row S22.
    #[tokio::test]
    async fn an_oversized_view_ends_the_subscription_as_too_large() {
        let deliveries = Arc::new(SubscriptionDeliveries::new(Arc::new(
            EventSequence::default(),
        )));
        assert!(deliveries.reserve("1"));
        let mut sent = sent(&deliveries);
        let view = Value::from("x".repeat(MAX_PAYLOAD_BYTES as usize));
        let frame = Frame {
            payload: serde_json::json!({ "subscriptionId": "1", "view": view.clone() }),
            cursor: Some(Cursor {
                incarnation: "i".into(),
                position: 1,
            }),
            key: view,
            more: false,
        };
        assert_eq!(sent.deliver_frame(frame).await, None);
        assert_eq!(ended(&deliveries)["reason"], "too_large");
    }

    /// Row S22 at the boundary: the largest payload `fits` admits, framed
    /// with the largest sequence, is within the bound the writer checks.
    #[test]
    fn the_largest_payload_that_fits_is_within_the_writers_frame_bound() {
        let room = MAX_PAYLOAD_BYTES as usize - envelope_bytes() - "\"\"".len();
        let largest = Value::from("x".repeat(room));
        assert!(fits(&largest));
        assert!(!fits(&Value::from("x".repeat(room + 1))));
        for name in [
            product_event::CONVERSATION_VIEW,
            product_event::CONVERSATION_LISTED,
        ] {
            let frame = EventFrame::push(name, &largest, u64::MAX, u64::MAX).unwrap();
            let text = OutgoingMessage::Event(frame).to_wire_text().unwrap();
            assert!(text.len() <= MAX_PAYLOAD_BYTES as usize, "{name}");
        }
    }

    /// Row S10 at the delivery level: an offer nobody takes is withdrawn
    /// and the subscription ends lagging with its last written cursor.
    #[tokio::test(start_paused = true)]
    async fn an_untaken_frame_ends_lagging_with_the_last_written_cursor() {
        let deliveries = Arc::new(SubscriptionDeliveries::new(Arc::new(
            EventSequence::default(),
        )));
        assert!(deliveries.reserve("1"));
        let mut sent = sent(&deliveries);
        sent.floor = Some(Cursor {
            incarnation: "i".into(),
            position: 7,
        });
        let offered = sent.offer(
            Value::from(1),
            Some(Cursor {
                incarnation: "i".into(),
                position: 9,
            }),
            Value::from(1),
        );
        assert!(!offered.await, "nobody took it");
        let payload = ended(&deliveries);
        assert_eq!(payload["reason"], "lagging");
        assert_eq!(payload["lastDelivered"]["position"], "7");
    }

    /// Row S24: the writer took the frame and is still writing it when its
    /// delivery deadline passes. It is written, and nothing ends.
    #[tokio::test(start_paused = true)]
    async fn a_frame_taken_as_its_deadline_passes_is_written_and_ends_nothing() {
        let deliveries = Arc::new(SubscriptionDeliveries::new(Arc::new(
            EventSequence::default(),
        )));
        assert!(deliveries.reserve("1"));
        let mut sent = sent(&deliveries);
        {
            let offered = sent.offer(
                Value::from(1),
                Some(Cursor {
                    incarnation: "i".into(),
                    position: 9,
                }),
                Value::from(1),
            );
            tokio::pin!(offered);
            assert!(tokio::time::timeout(Duration::ZERO, &mut offered)
                .await
                .is_err());
            let frame = deliveries.take().expect("the offer");
            tokio::time::advance(DELIVERY_TIMEOUT * 2).await;
            assert!(
                tokio::time::timeout(Duration::ZERO, &mut offered)
                    .await
                    .is_err(),
                "still waiting on the write it cannot withdraw"
            );
            frame.written.told();
            deliveries.sent("1", false);
            assert!(offered.await, "written");
        }
        assert!(deliveries.take().is_none(), "no lagging end offered");
        assert_eq!(sent.floor.map(|cursor| cursor.position), Some(9));
    }

    /// Row S25.
    #[tokio::test(start_paused = true)]
    async fn a_first_read_past_the_reply_deadline_is_refused_unavailable() {
        let reply_by = Instant::now() + Duration::from_secs(1);
        let read = by_reply::<()>(reply_by, std::future::pending());
        assert_eq!(read.await, Err(code_of(&ConversationError::Unavailable)),);
        let quick = by_reply(reply_by + Duration::from_secs(1), async { Ok(1) });
        assert_eq!(quick.await, Ok(1));
    }

    /// Row S9: two reads that differ only in their revision are the same
    /// view to the client.
    #[test]
    fn views_that_differ_only_in_revision_have_one_key() {
        let first = serde_json::json!({"revision": "a:1", "messages": [1]});
        let renumbered = serde_json::json!({"revision": "b:1", "messages": [1]});
        let changed = serde_json::json!({"revision": "a:1", "messages": [2]});
        assert_eq!(view_key(&first), view_key(&renumbered));
        assert_ne!(view_key(&first), view_key(&changed));
    }

    /// Row S23.
    #[test]
    fn a_read_without_a_storage_slot_is_tried_again_then_refused() {
        let busy = ConversationError::Storage(StorageError::Busy);
        let full = ConversationError::Storage(StorageError::ReadCapacity);
        assert_eq!(retry_after(&busy, 0), Some(TRANSIENT_BACKOFF));
        assert_eq!(retry_after(&full, 1), Some(TRANSIENT_BACKOFF * 2));
        assert_eq!(
            retry_after(&busy, TRANSIENT_RETRIES - 1),
            Some(TRANSIENT_BACKOFF_CAP)
        );
        assert_eq!(retry_after(&busy, TRANSIENT_RETRIES), None);
        assert_eq!(retry_after(&ConversationError::Unavailable, 0), None);
    }

    async fn waits(sources: &mut Sources) -> bool {
        tokio::time::timeout(Duration::ZERO, sources.changed())
            .await
            .is_err()
    }

    /// Row L5: notices that wait together are taken before a list reads, so
    /// a commit that dirtied two sources is one read; whether the catalogue
    /// was among them is said, and nothing taken is left to wake it again.
    #[tokio::test]
    async fn a_list_takes_every_waiting_notice_before_it_reads() {
        use crate::conversation::application::catalogue_watch::{
            CatalogueChangeSignal, CatalogueWatchRegistration,
        };
        struct Unregistered;
        impl CatalogueWatchRegistration for Unregistered {}
        let directory = tempfile::tempdir().unwrap();
        let changes = nessa_sdk::infrastructure::session_storage::RecordStorage::new(
            directory.path().join("records.sqlite"),
        )
        .unwrap();
        let signal = Arc::new(CatalogueChangeSignal::default());
        let (live, receiver) = watch::channel(0u64);
        let mut sources = Sources {
            records: changes.watch_any_committed().unwrap(),
            catalogue: Some(CatalogueChangeWatch::new(
                signal.clone(),
                Box::new(Unregistered),
            )),
            live: Some(receiver),
        };
        assert!(!sources.drain().await, "nothing waiting");
        live.send(1).unwrap();
        signal.publish();
        assert!(sources.drain().await, "the catalogue's notice is said");
        assert!(waits(&mut sources).await, "both taken");
        live.send(2).unwrap();
        assert!(
            !sources.drain().await,
            "another source's alone is not the catalogue's"
        );
        assert!(waits(&mut sources).await);
        drop(changes);
    }

    /// Row S19 at the source level.
    #[tokio::test]
    async fn a_closed_source_ends_the_subscription() {
        let directory = tempfile::tempdir().unwrap();
        let changes = nessa_sdk::infrastructure::session_storage::RecordStorage::new(
            directory.path().join("records.sqlite"),
        )
        .unwrap();
        let records = changes.watch_any_committed().unwrap();
        let mut sources = Sources {
            records,
            catalogue: None,
            live: None,
        };
        drop(changes);
        assert!(matches!(sources.changed().await, Wake::Closed));
    }
}
