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

use super::delivery::{Outgoing, SubscriptionDeliveries};
use crate::conversation::application::{
    error_code, CatalogueChangeWatch, CatalogueWatchError, CatalogueWatchState, ConversationError,
    ConversationService,
};
use crate::product::{
    conversation::{caller, read_list, read_view},
    passive_read::deadlines::RECORD_SEND_TIMEOUT,
    socket::{admit_action, current_session_now, failure, success},
    state::ProductRouteState,
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
pub(super) const DELIVERY_TIMEOUT: Duration = Duration::from_millis(SUBSCRIPTION_DELIVERY_TIMEOUT_MS);

/// Room an event's envelope takes beside its payload: type, name, sequence
/// and state version, with a twenty-digit sequence.
const ENVELOPE_BYTES: usize = 256;

/// After a wake, a list waits this long before it reads, so a stream of
/// commits (a reply being saved every 100 ms) costs a few list reads a
/// second, not one per commit. Changes during the wait still collapse into
/// the next read. A view never waits.
const LIST_REREAD_FLOOR: Duration = Duration::from_millis(250);

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
            (Self::View { conversation, .. }, Self::View { conversation: other, .. }) => {
                conversation == other
            }
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
            ChangeWatchError::Capacity => {
                ConversationSubscriptionErrorCode::SubscriptionCapacity
                    .as_str()
                    .to_owned()
            }
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
                CatalogueWatchState::Dirty => Wake::Changed,
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
    let current = current_session_now(state, session).await?;
    admit_action(state, &current, target.method()).await?;
    Ok(current)
}

/// One read's result: a view and where it was folded through, or a list.
enum Batch {
    View {
        view: ConversationView,
        cursor: Cursor,
        /// The history's head as that read observed it.
        head: u64,
    },
    List(ConversationListResult),
}

fn code_of(error: &ConversationError) -> String {
    error_code(error).as_str().to_owned()
}

/// Refused for want of a storage read slot: tried again (`TRANSIENT_RETRIES`).
fn transient(error: &ConversationError) -> bool {
    matches!(
        error,
        ConversationError::Storage(StorageError::ReadCapacity | StorageError::Busy)
    )
}

async fn read_batch(
    state: &ProductRouteState,
    service: &ConversationService,
    session: &AuthenticatedSession,
    target: &Target,
    request_id: &str,
) -> Result<Batch, String> {
    let mut backoff = TRANSIENT_BACKOFF;
    let mut attempt = 0;
    loop {
        let current = authorize_batch(state, session, target).await?;
        // A batch is a request of the gateway's like any other.
        let permit = state.requests.clone().acquire_owned().await;
        let read = match target {
            Target::View { conversation, .. } => {
                match read_view(state, service, &current, conversation, request_id).await {
                    Ok((view, Some(cursor))) => Ok(Batch::View {
                        view,
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
            Err(error) if transient(&error) && attempt < TRANSIENT_RETRIES => {
                attempt += 1;
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(TRANSIENT_BACKOFF_CAP);
            }
            Err(error) => return Err(code_of(&error)),
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
    /// the passive delivery budget, which a client's call outlasts.
    pub reply_by: Instant,
}

impl Run {
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
            None,
            None,
        );
        tokio::pin!(written);
        let delivered = tokio::select! {
            result = &mut written => result.is_ok(),
            () = tokio::time::sleep(DELIVERY_TIMEOUT) => {
                if self.deliveries.withdraw(&self.id) {
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
            Some(Instant::now() + DELIVERY_TIMEOUT),
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
                let more = view.transcript_state == "stale"
                    && self.read_through.as_ref() != Some(&cursor);
                self.read_through = Some(cursor.clone());
                // Never a frame behind what the client has (row S5).
                if self.floor.as_ref().is_some_and(|floor| cursor.behind(floor)) {
                    return Framed::Behind { more };
                }
                let payload = serde_json::to_value(ConversationViewed {
                    subscription_id: self.id.clone(),
                    cursor: cursor.wire(),
                    view,
                })
                .expect("generated payload serializes");
                let key = payload["view"].clone();
                Framed::Frame(Frame {
                    payload,
                    cursor: Some(cursor),
                    key,
                    more,
                })
            }
            Batch::List(list) => {
                let payload = serde_json::to_value(ConversationListed {
                    subscription_id: self.id.clone(),
                    list: fitted(list, &self.id),
                })
                .expect("generated payload serializes");
                let key = payload["list"].clone();
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

/// Whether an event with `payload` fits the socket's frame bound.
fn fits(payload: &Value) -> bool {
    serde_json::to_string(payload)
        .map(|text| text.len() + ENVELOPE_BYTES <= MAX_PAYLOAD_BYTES as usize)
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
    let budget = (MAX_PAYLOAD_BYTES as usize).saturating_sub(ENVELOPE_BYTES);
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
        reply_by,
    } = run;
    let refuse = |code: &str, slot: Arc<OwnedSemaphorePermit>| {
        drop(deliveries.offer(
            &id,
            Outgoing::Reply(failure(&request, code)),
            Some(Instant::now() + DELIVERY_TIMEOUT),
            Some(slot),
        ));
    };
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
    };
    let mut reply = Some(slot);
    loop {
        let batch = if reply.is_some() {
            timeout_at(
                reply_by,
                read_batch(&state, &service, &session, &target, &request),
            )
            .await
            .unwrap_or_else(|_| Err(code_of(&ConversationError::Unavailable)))
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
                // written before any event of it (row S1).
                if deliveries
                    .offer(&id, Outgoing::Reply(answer), None, Some(slot))
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
            Wake::Closed => {
                sent.end(ConversationSubscriptionEndReason::SourceClosed, None);
                return;
            }
        }
        if matches!(target, Target::List { .. }) {
            tokio::time::sleep(LIST_REREAD_FLOOR).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::product::event_sequence::EventSequence;
    use nessa_protocol::product::generated::ConversationSummary;
    use nessa_protocol::protocol::OutgoingMessage;

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

    fn sent(deliveries: &Arc<SubscriptionDeliveries>) -> Sent {
        Sent {
            deliveries: deliveries.clone(),
            id: "1".into(),
            floor: None,
            last: None,
            read_through: None,
        }
    }

    fn ended(deliveries: &SubscriptionDeliveries) -> Value {
        let frame = deliveries.take().expect("a terminal frame");
        assert!(frame.terminal.is_some());
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
