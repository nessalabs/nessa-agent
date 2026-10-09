//! One connection's subscriptions: what each follows, its task, and its
//! place on the way to the writer. The connection owns them; they go with it
//! (row S21).

use super::{
    delivery::SubscriptionDeliveries,
    target::{run, Cursor, Run, Target},
};
use crate::product::{
    conversation::conversation_id,
    event_sequence::EventSequence,
    socket::{failure, success, valid_product_request},
    state::{note_limit, ProductRouteState},
};
use nessa_auth::application::session::AuthenticatedSession;
use nessa_protocol::product::generated::{
    product_method, ConversationSubscribeListParams, ConversationSubscribeParams,
    ConversationSubscribeResult, ConversationSubscriptionErrorCode, ConversationUnsubscribeParams,
    MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS, MAX_CONNECTION_LIST_SUBSCRIPTIONS,
};
use nessa_protocol::product_contract::generated::ConversationErrorCode;
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};
use std::sync::Arc;
use tokio::{sync::OwnedSemaphorePermit, task::AbortHandle, time::Instant};

struct Entry {
    id: String,
    target: Target,
    task: AbortHandle,
}

pub(in crate::product) struct ConnectionSubscriptions {
    entries: Vec<Entry>,
    minted: u64,
    pub deliveries: Arc<SubscriptionDeliveries>,
}

impl ConnectionSubscriptions {
    /// Events continue `sequence`, the socket's one numbering.
    pub fn new(sequence: Arc<EventSequence>) -> Self {
        Self {
            entries: Vec::with_capacity(
                MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS + MAX_CONNECTION_LIST_SUBSCRIPTIONS,
            ),
            minted: 0,
            deliveries: Arc::new(SubscriptionDeliveries::new(sequence)),
        }
    }

    /// The methods this owns, by their generated names.
    pub fn method(method: &str) -> bool {
        matches!(
            method,
            product_method::CONVERSATION_SUBSCRIBE
                | product_method::CONVERSATION_SUBSCRIBE_LIST
                | product_method::CONVERSATION_UNSUBSCRIBE
        )
    }

    /// A subscription is live while its task runs or its last frame waits
    /// for the writer; one that has gone frees its place.
    fn collect(&mut self) {
        let deliveries = &self.deliveries;
        self.entries
            .retain(|entry| !entry.task.is_finished() || deliveries.has(&entry.id));
    }

    /// Whether `method` stops a subscription: the socket admits it before
    /// [`Self::begin`] is asked (row S29).
    pub fn stops(method: &str) -> bool {
        method == product_method::CONVERSATION_UNSUBSCRIBE
    }

    /// Begin, or stop, one subscription. The answer for the ordinary lane, or
    /// `None` when the subscription's task answers through its own place,
    /// after its first read (row S1). Nothing here admits the request: the
    /// socket admitted an unsubscribe before asking, and a subscription's
    /// task admits it before registering anything and again for each batch
    /// (`authorize_batch`, row S29).
    pub fn begin(
        &mut self,
        state: &ProductRouteState,
        session: &AuthenticatedSession,
        frame: RequestFrame,
        slot: Arc<OwnedSemaphorePermit>,
        received_at: Instant,
    ) -> Option<OutgoingMessage> {
        let invalid = || failure(&frame.id, ConversationErrorCode::InvalidRequest.as_str());
        if !valid_product_request(&frame) {
            return Some(invalid());
        }
        self.collect();
        if frame.method == product_method::CONVERSATION_UNSUBSCRIBE {
            let Ok(params) =
                serde_json::from_value::<ConversationUnsubscribeParams>(frame.params.clone())
            else {
                return Some(invalid());
            };
            let Some(position) = self
                .entries
                .iter()
                .position(|entry| entry.id == params.subscription_id)
            else {
                return Some(failure(
                    &frame.id,
                    ConversationSubscriptionErrorCode::UnknownSubscription.as_str(),
                ));
            };
            let entry = self.entries.remove(position);
            // Retired before the reply is queued: an offer the writer has not
            // taken is never written, so no frame follows the reply (row S16).
            self.deliveries.retire(&entry.id);
            entry.task.abort();
            return Some(success(
                &frame.id,
                &ConversationSubscribeResult {
                    subscription_id: entry.id,
                },
            ));
        }
        let target = if frame.method == product_method::CONVERSATION_SUBSCRIBE {
            let Ok(params) =
                serde_json::from_value::<ConversationSubscribeParams>(frame.params.clone())
            else {
                return Some(invalid());
            };
            let Ok(conversation) = conversation_id(&params.conversation_id) else {
                return Some(invalid());
            };
            let after = match params.after.as_ref().map(Cursor::decode) {
                Some(None) => return Some(invalid()),
                Some(Some(after)) => Some(after),
                None => None,
            };
            Target::View {
                conversation,
                after,
            }
        } else {
            let Ok(params) =
                serde_json::from_value::<ConversationSubscribeListParams>(frame.params.clone())
            else {
                return Some(invalid());
            };
            Target::List {
                archived: params.archived.unwrap_or(false),
            }
        };
        let Some(service) = state.conversations.clone() else {
            return Some(failure(
                &frame.id,
                ConversationErrorCode::ConversationsNotConfigured.as_str(),
            ));
        };
        if self.entries.iter().any(|entry| entry.target.same(&target)) {
            return Some(failure(
                &frame.id,
                ConversationSubscriptionErrorCode::SubscriptionDuplicate.as_str(),
            ));
        }
        let (limit, of_kind, named) = match target {
            Target::View { .. } => (
                MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS,
                self.entries
                    .iter()
                    .filter(|entry| matches!(entry.target, Target::View { .. }))
                    .count(),
                "socket.conversation_subscriptions",
            ),
            Target::List { .. } => (
                MAX_CONNECTION_LIST_SUBSCRIPTIONS,
                self.entries
                    .iter()
                    .filter(|entry| matches!(entry.target, Target::List { .. }))
                    .count(),
                "socket.list_subscriptions",
            ),
        };
        let capacity = || {
            failure(
                &frame.id,
                ConversationSubscriptionErrorCode::SubscriptionCapacity.as_str(),
            )
        };
        if of_kind >= limit {
            note_limit(named);
            return Some(capacity());
        }
        let Some(minted) = self.minted.checked_add(1) else {
            return Some(capacity());
        };
        let id = minted.to_string();
        if !self.deliveries.reserve(&id) {
            return Some(capacity());
        }
        self.minted = minted;
        let task = tokio::spawn(run(Run::new(
            state.clone(),
            service,
            session.clone(),
            id.clone(),
            frame.id,
            target.clone(),
            self.deliveries.clone(),
            slot,
            received_at,
        )));
        self.entries.push(Entry {
            id,
            target,
            task: task.abort_handle(),
        });
        None
    }
}

impl Drop for ConnectionSubscriptions {
    fn drop(&mut self) {
        // Each task's sources and reads go with it (row S21).
        for entry in &self.entries {
            entry.task.abort();
        }
        self.deliveries.close();
    }
}
