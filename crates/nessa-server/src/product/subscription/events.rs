//! Committed event delivery: one retained SDK reader per socket subscription.
//! Only payloadless source notices coalesce; semantic events do not.

use super::{
    delivery::{Deadline, Outgoing, SubscriptionDeliveries},
    target::{authorize_batch, backoff, Run, Sources, Target, Wake, DELIVERY_TIMEOUT},
};
use crate::conversation::application::{
    conversation_session, event_error_code, project_events, validate_event_cursor,
    ConversationEventError, ConversationService, EventProgress, ThreadEvent,
};
use crate::product::conversation::caller;
use crate::product::socket::{failure, success};
use crate::product::ProductRouteState;
use nessa_auth::application::session::AuthenticatedSession;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::product::generated::{
    product_event, ConversationSubscribeResult, MAX_PAYLOAD_BYTES,
};
use nessa_protocol::product_contract::generated::ConversationErrorCode;
use nessa_protocol::protocol::{EventFrame, OutgoingMessage};
use nessa_sdk::application::agent_execution::sessions::{
    CommittedEventCursor, CommittedEventReader, EventRead, EventReadError, EventReadLimits,
    EventStart,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::time::{timeout_at, Instant};

/// Run an event target using the connection's existing delivery owner.
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
    let mut slot = Some(slot);
    let Target::Events {
        conversation,
        after,
        replay,
        kinds,
    } = &target
    else {
        return;
    };
    macro_rules! refuse {
        ($code:expr) => {{
            drop(deliveries.offer(
                &id,
                Outgoing::Reply(failure(&request, $code)),
                Deadline::Last(Instant::now() + DELIVERY_TIMEOUT),
                slot.take(),
            ));
            return;
        }};
    }
    if let Err(code) = authorize_batch(&state, &session, &target).await {
        refuse!(code);
    }
    let Some(service) = service.as_ref() else {
        refuse!("conversations_not_configured");
    };
    let mut sources = match Sources::register(&state, Some(service), &session, &target, None) {
        Ok(sources) => sources,
        Err(code) => refuse!(&code),
    };
    let start = match after {
        Some(token) => {
            match CommittedEventCursor::decode(&conversation_session(conversation), token) {
                Ok(cursor) => EventStart::After(cursor),
                Err(_) => refuse!("event_cursor_invalid"),
            }
        }
        None if *replay => EventStart::Beginning,
        None => EventStart::Now,
    };
    let mut progress = EventProgress::new(&start);
    let owner = caller(&session, request.clone());
    let mut reader = match timeout_at(
        read_by,
        service.open_events(conversation.clone(), owner.clone(), start),
    )
    .await
    {
        Ok(Ok(reader)) => reader,
        Ok(Err(error)) => refuse!(event_error_code(&error)),
        Err(_) => refuse!("unavailable"),
    };
    let mut replied = false;
    let mut last = after.clone();
    // A logical saved fact can span many physical frames; this is the SDK's
    // bounded semantic-input budget, not the much smaller metadata wire frame.
    let limits = EventReadLimits::new(16, 256 * 1024 * 1024).expect("fixed SDK page limits");
    loop {
        let read = read_event(
            &state,
            service,
            &session,
            &target,
            &request,
            reader.as_mut(),
            limits,
        );
        let result = if replied {
            read.await
        } else {
            timeout_at(read_by, read)
                .await
                .unwrap_or_else(|_| Err("unavailable".to_owned()))
        };
        let result = match result {
            Ok(result) => result,
            Err(code) => {
                if !replied {
                    refuse!(&code);
                }
                end(&deliveries, &id, "refused", Some(&code), last.as_deref());
                return;
            }
        };
        if let Err(error) = progress.accept(&result) {
            let code = event_error_code(&ConversationEventError::Events(error));
            if !replied {
                refuse!(code);
            }
            end(&deliveries, &id, "refused", Some(code), last.as_deref());
            return;
        }
        let (mut events, cursor, idle) = match result {
            EventRead::Preparing => {
                tokio::task::yield_now().await;
                continue;
            }
            EventRead::Page(page) => match validate_event_cursor(conversation, page.next())
                .and_then(|()| validate_event_cursor(conversation, page.captured_head()))
                .and_then(|()| project_events(conversation, page.events()))
            {
                Ok(events) => (events, page.next().encode(), false),
                Err(error) => {
                    if !replied {
                        refuse!(event_error_code(&error));
                    }
                    end(
                        &deliveries,
                        &id,
                        "refused",
                        Some(event_error_code(&error)),
                        last.as_deref(),
                    );
                    return;
                }
            },
            EventRead::Idle(cursor) => {
                if let Err(error) = validate_event_cursor(conversation, &cursor) {
                    if !replied {
                        refuse!(event_error_code(&error));
                    }
                    end(
                        &deliveries,
                        &id,
                        "refused",
                        Some(event_error_code(&error)),
                        last.as_deref(),
                    );
                    return;
                }
                (Vec::new(), cursor.encode(), true)
            }
        };
        if !replied {
            let answer = success(
                &request,
                &ConversationSubscribeResult {
                    subscription_id: id.clone(),
                },
            );
            if deliveries
                .offer(
                    &id,
                    Outgoing::Reply(answer),
                    Deadline::Reply(reply_by),
                    slot.take(),
                )
                .await
                .is_err()
            {
                return;
            }
            replied = true;
        }
        events.retain(|event| kinds.is_empty() || kinds.iter().any(|kind| kind == event.kind));
        // A page whose events all stay unpublished (streamed text, or kinds
        // not asked for) sends nothing until the head: a long reply would
        // otherwise send an empty frame per page, and a follower takes an
        // empty frame as a hint that it has caught up.
        if last.as_deref() != Some(&cursor) && (idle || !events.is_empty()) {
            let payload = frame(&id, &cursor, events);
            let encoded = EventFrame::push(
                product_event::CONVERSATION_EVENTS,
                &payload,
                u64::MAX,
                u64::MAX,
            )
            .ok()
            .and_then(|event| OutgoingMessage::Event(event).to_wire_text().ok());
            if encoded
                .as_ref()
                .is_none_or(|text| text.len() > MAX_PAYLOAD_BYTES as usize)
            {
                end(&deliveries, &id, "too_large", None, last.as_deref());
                return;
            }
            if !offer_event(&deliveries, &id, payload, last.as_deref()).await {
                return;
            }
            last = Some(cursor);
        }
        if idle {
            match sources.changed().await {
                Wake::Changed | Wake::Catalogue => {}
                Wake::Closed => {
                    end(&deliveries, &id, "source_closed", None, last.as_deref());
                    return;
                }
            }
        }
    }
}

/// Capacity precedes fresh admission and source I/O for every semantic batch.
pub(in crate::product) async fn read_event(
    state: &ProductRouteState,
    service: &ConversationService,
    session: &AuthenticatedSession,
    target: &Target,
    request: &str,
    reader: &mut dyn CommittedEventReader,
    limits: EventReadLimits,
) -> Result<EventRead, String> {
    let Target::Events { conversation, .. } = target else {
        return Err("invalid_request".into());
    };
    let mut attempt = 0;
    loop {
        let permit = state
            .requests
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "unavailable".to_owned())?;
        let (current, _) = match authorize_batch(state, session, target).await {
            Ok(current) => current,
            Err(code) => match backoff(
                code == ConversationErrorCode::TemporarilyUnavailable.as_str(),
                attempt,
            ) {
                Some(wait) => {
                    drop(permit);
                    attempt += 1;
                    tokio::time::sleep(wait).await;
                    continue;
                }
                None => return Err(code.to_owned()),
            },
        };
        authorize_event_target(service, &current, conversation, request).await?;
        let result = reader.read(limits).await;
        // E22: source I/O may outlive credential or target authority. Admit the
        // returned data/checkpoint for publication while capacity is still held.
        if matches!(&result, Ok(EventRead::Page(_) | EventRead::Idle(_))) {
            let (current, _) = authorize_batch(state, session, target)
                .await
                .map_err(str::to_owned)?;
            authorize_event_target(service, &current, conversation, request).await?;
        }
        drop(permit);
        match result {
            Err(error) => match backoff(matches!(error, EventReadError::Capacity), attempt) {
                Some(wait) => {
                    attempt += 1;
                    tokio::time::sleep(wait).await;
                }
                None => {
                    return Err(event_error_code(&ConversationEventError::Events(error)).to_owned())
                }
            },
            Ok(read) => return Ok(read),
        }
    }
}

/// Ask the application target owner using the freshly admitted session.
async fn authorize_event_target(
    service: &ConversationService,
    session: &AuthenticatedSession,
    conversation: &ConversationId,
    request: &str,
) -> Result<(), String> {
    service
        .authorize_events(conversation, &caller(session, request.to_owned()))
        .await
        .map_err(|error| event_error_code(&ConversationEventError::Conversation(error)).to_owned())
}

async fn offer_event(
    deliveries: &Arc<SubscriptionDeliveries>,
    id: &str,
    payload: Value,
    last: Option<&str>,
) -> bool {
    let written = deliveries.offer(
        id,
        Outgoing::Event {
            name: product_event::CONVERSATION_EVENTS,
            payload,
        },
        Deadline::None,
        None,
    );
    tokio::pin!(written);
    tokio::select! {
        result=&mut written=>result.is_ok(),
        ()=tokio::time::sleep(DELIVERY_TIMEOUT)=>{
            if deliveries.withdraw(id) { end(deliveries,id,"lagging",None,last); return false; }
            (&mut written).await.is_ok()
        }
    }
}

fn frame(id: &str, cursor: &str, events: Vec<ThreadEvent>) -> Value {
    json!({"subscriptionId": id, "cursor": cursor, "events": events})
}

fn end(
    deliveries: &Arc<SubscriptionDeliveries>,
    id: &str,
    reason: &str,
    code: Option<&str>,
    last: Option<&str>,
) {
    let mut payload = json!({"subscriptionId": id, "reason": reason});
    if let Some(code) = code {
        payload["code"] = json!(code);
    }
    if let Some(cursor) = last {
        payload["lastEventCursor"] = json!(cursor);
    }
    drop(deliveries.offer(
        id,
        Outgoing::Event {
            name: product_event::CONVERSATION_SUBSCRIPTION_ENDED,
            payload,
        },
        Deadline::Last(Instant::now() + DELIVERY_TIMEOUT),
        None,
    ));
}

#[cfg(test)]
#[path = "../../../tests/product/subscription_event_delivery.rs"]
mod tests;
