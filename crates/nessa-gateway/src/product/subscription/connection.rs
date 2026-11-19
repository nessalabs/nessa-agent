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
    product_method, ConversationSubscribeEventsParams, ConversationSubscribeListParams,
    ConversationSubscribeParams, ConversationSubscribeResult, ConversationSubscriptionErrorCode,
    ConversationUnsubscribeParams, GrantStreamCursor, GrantsPageParams, GrantsSubscribeParams,
    GrantsUnsubscribeParams, MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS,
    MAX_CONNECTION_GRANT_SUBSCRIPTIONS, MAX_CONNECTION_LIST_SUBSCRIPTIONS,
    MAX_EVENT_SUBSCRIPTION_KINDS, MIN_EVENT_SUBSCRIPTION_KINDS,
    NON_NULL_EVENT_SUBSCRIPTION_OPTIONS, UNIQUE_EVENT_SUBSCRIPTION_KINDS,
};
use nessa_protocol::product::passive_read::decimal_u64;
use nessa_protocol::product_contract::generated::ConversationErrorCode;
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};
use serde_json::json;
use std::sync::Arc;
use tokio::{sync::mpsc, sync::OwnedSemaphorePermit, task::AbortHandle, time::Instant};

struct Entry {
    id: String,
    target: Target,
    task: AbortHandle,
    pages: Option<mpsc::Sender<super::grants::GrantPage>>,
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
            // One more for the agent catalog.
            entries: Vec::with_capacity(
                MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS + MAX_CONNECTION_LIST_SUBSCRIPTIONS + 1,
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
                | product_method::CONVERSATION_SUBSCRIBE_EVENTS
                | product_method::CONVERSATION_SUBSCRIBE_LIST
                | product_method::CONVERSATION_UNSUBSCRIBE
                | product_method::AGENTS_SUBSCRIBE
                | product_method::AGENTS_UNSUBSCRIBE
                | product_method::GRANTS_SUBSCRIBE
                | product_method::GRANTS_PAGE
                | product_method::GRANTS_UNSUBSCRIBE
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
        matches!(
            method,
            product_method::CONVERSATION_UNSUBSCRIBE
                | product_method::AGENTS_UNSUBSCRIBE
                | product_method::GRANTS_UNSUBSCRIBE
        )
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
        if frame.method == product_method::GRANTS_PAGE {
            let Ok(params) = serde_json::from_value::<GrantsPageParams>(frame.params.clone())
            else {
                return Some(invalid());
            };
            let Some(entry) = self
                .entries
                .iter()
                .find(|entry| entry.id == params.subscription_id.as_str())
            else {
                return Some(failure(
                    &frame.id,
                    ConversationSubscriptionErrorCode::UnknownSubscription.as_str(),
                ));
            };
            let Some(pages) = &entry.pages else {
                return Some(failure(
                    &frame.id,
                    ConversationSubscriptionErrorCode::UnknownSubscription.as_str(),
                ));
            };
            let after = match params.after.as_ref().map(grant_cursor) {
                Some(None) => return Some(invalid()),
                after => after.flatten(),
            };
            if pages
                .try_send(super::grants::GrantPage {
                    snapshot_id: params.snapshot_id,
                    after,
                })
                .is_err()
            {
                return Some(failure(
                    &frame.id,
                    ConversationSubscriptionErrorCode::UnknownSubscription.as_str(),
                ));
            }
            return Some(success(
                &frame.id,
                &ConversationSubscribeResult {
                    subscription_id: params.subscription_id,
                },
            ));
        }
        if Self::stops(&frame.method) {
            let subscription_id = if frame.method == product_method::GRANTS_UNSUBSCRIBE {
                let Ok(params) =
                    serde_json::from_value::<GrantsUnsubscribeParams>(frame.params.clone())
                else {
                    return Some(invalid());
                };
                params.subscription_id
            } else {
                let Ok(params) =
                    serde_json::from_value::<ConversationUnsubscribeParams>(frame.params.clone())
                else {
                    return Some(invalid());
                };
                params.subscription_id
            };
            // Each unsubscribe stops only what the subscribe beside it made,
            // whose grant it asks (row AC7).
            let Some(position) = self.entries.iter().position(|entry| {
                entry.id == subscription_id && entry.target.stopped_by() == frame.method
            }) else {
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
        let target = if frame.method == product_method::GRANTS_SUBSCRIBE {
            let Ok(params) = serde_json::from_value::<GrantsSubscribeParams>(frame.params.clone())
            else {
                return Some(invalid());
            };
            let after = match params.after.as_ref().map(grant_cursor) {
                Some(None) => return Some(invalid()),
                after => after.flatten(),
            };
            Target::Grants { after }
        } else if frame.method == product_method::AGENTS_SUBSCRIBE {
            // Asked as `agents.list` is: no parameters.
            if frame.params != json!({}) {
                return Some(invalid());
            }
            Target::Agents
        } else if frame.method == product_method::CONVERSATION_SUBSCRIBE_EVENTS {
            if NON_NULL_EVENT_SUBSCRIPTION_OPTIONS.iter().any(|field| {
                frame
                    .params
                    .get(*field)
                    .is_some_and(serde_json::Value::is_null)
            }) {
                return Some(invalid());
            }
            let Ok(params) =
                serde_json::from_value::<ConversationSubscribeEventsParams>(frame.params.clone())
            else {
                return Some(invalid());
            };
            let Ok(conversation) = conversation_id(&params.conversation_id) else {
                return Some(invalid());
            };
            if params.after.is_some() && params.replay.is_some() {
                return Some(invalid());
            }
            if let Some(kinds) = &params.kinds {
                if !(MIN_EVENT_SUBSCRIPTION_KINDS..=MAX_EVENT_SUBSCRIPTION_KINDS)
                    .contains(&kinds.len())
                    || (UNIQUE_EVENT_SUBSCRIPTION_KINDS
                        && kinds
                            .iter()
                            .enumerate()
                            .any(|(index, kind)| kinds[..index].contains(kind)))
                {
                    return Some(invalid());
                }
            }
            Target::Events {
                conversation,
                after: params.after,
                replay: params.replay.unwrap_or(false),
                kinds: params
                    .kinds
                    .unwrap_or_default()
                    .into_iter()
                    .map(|kind| kind.as_str().to_owned())
                    .collect(),
            }
        } else if frame.method == product_method::CONVERSATION_SUBSCRIBE {
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
        let not_configured = || {
            Some(failure(
                &frame.id,
                ConversationErrorCode::ConversationsNotConfigured.as_str(),
            ))
        };
        // The agent catalog reads no conversation, and is composed where
        // conversations are, as `agents.list` answers.
        let service = state.conversation.service.clone();
        match target {
            Target::Agents if state.agents.catalog.is_none() => return not_configured(),
            Target::View { .. } | Target::List { .. } | Target::Events { .. }
                if service.is_none() =>
            {
                return not_configured()
            }
            _ => {}
        }
        let capacity = || {
            failure(
                &frame.id,
                ConversationSubscriptionErrorCode::SubscriptionCapacity.as_str(),
            )
        };
        if matches!(target, Target::Grants { .. })
            && self
                .entries
                .iter()
                .filter(|entry| matches!(entry.target, Target::Grants { .. }))
                .count()
                >= MAX_CONNECTION_GRANT_SUBSCRIPTIONS
        {
            note_limit("socket.grant_subscriptions");
            return Some(capacity());
        }
        if self.entries.iter().any(|entry| entry.target.same(&target)) {
            return Some(failure(
                &frame.id,
                ConversationSubscriptionErrorCode::SubscriptionDuplicate.as_str(),
            ));
        }
        let capacity = || {
            failure(
                &frame.id,
                ConversationSubscriptionErrorCode::SubscriptionCapacity.as_str(),
            )
        };
        // The agent catalog is one target, so the duplicate refusal above is
        // its bound: one per connection.
        let bound = match target {
            Target::View { .. } | Target::Events { .. } => Some((
                MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS,
                self.entries
                    .iter()
                    .filter(|entry| {
                        matches!(entry.target, Target::View { .. } | Target::Events { .. })
                    })
                    .count(),
                "socket.conversation_subscriptions",
            )),
            Target::List { .. } => Some((
                MAX_CONNECTION_LIST_SUBSCRIPTIONS,
                self.entries
                    .iter()
                    .filter(|entry| matches!(entry.target, Target::List { .. }))
                    .count(),
                "socket.list_subscriptions",
            )),
            Target::Agents | Target::Grants { .. } => None,
        };
        if let Some((limit, of_kind, named)) = bound {
            if of_kind >= limit {
                note_limit(named);
                return Some(capacity());
            }
        }
        let Some(minted) = self.minted.checked_add(1) else {
            return Some(capacity());
        };
        let id = minted.to_string();
        if !self.deliveries.reserve(&id) {
            return Some(capacity());
        }
        self.minted = minted;
        let (page_sender, pages) = if matches!(target, Target::Grants { .. }) {
            let (sender, receiver) = mpsc::channel(4);
            (Some(sender), Some(receiver))
        } else {
            (None, None)
        };
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
            pages,
        )));
        self.entries.push(Entry {
            id,
            target,
            pages: page_sender,
            task: task.abort_handle(),
        });
        None
    }
}

fn grant_cursor(wire: &GrantStreamCursor) -> Option<Cursor> {
    Some(Cursor {
        incarnation: wire.incarnation.clone(),
        position: decimal_u64(&wire.position).ok()?,
    })
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
