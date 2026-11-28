//! An installed app's own session on the Product socket.
//!
//! An app signs in with the credential the owner issued it
//! (`apps.issueCredential`), on the same `/session` route and handshake as
//! any client, so `@nessa/client` works unchanged. What it may then do is
//! decided here and nowhere else:
//!
//! ```text
//! request ──▶ a method the app dispatcher routes at all? ────────── no ──▶ forbidden
//!         ──▶ its params decode? ─────────────────────────────────── no ──▶ invalid_request
//!         ──▶ AdmitAppCall: credential still current? ──────────── no ──▶ close
//!             Cedar `appCredential.call` admits the method, with
//!             what the request asks for (`full`, a channel, an
//!             environment, linked files), for the operations
//!             the owner approved? ─────────────────────────────── no ──▶ forbidden
//!         ──▶ conversation handler, as the app's own principal
//! ```
//!
//! The app's principal is its own (`app:<hash of owner, organization,
//! audience, app>`), so the conversation access policy admits it only to
//! conversations it created: never the owner's, never another app's. The
//! ordinary dispatcher is never reached, so nothing else on the socket
//! (terminals, windows, settings, credentials, notices) is served.
use std::time::Duration;

use axum::extract::ws::Message;
use axum::Error;
use futures_util::{Sink, Stream, StreamExt};
use nessa_auth::application::app_credential::AppCredentialBinding;
use nessa_auth::application::ports::{AccessError, CredentialEvidence};
use nessa_auth::application::session::{
    AdmitAppCall, AppCallAsk, AuthenticateAppSession, ReadCurrentAppSession, ScopedAppSession,
};
use nessa_protocol::product::generated::{ApprovalMode, ConversationCreateParams};
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame, ResponseFrame, MAX_PAYLOAD_BYTES};
use nessa_sdk::domain::agent_execution::executions::Initiation;
use serde_json::Value;
use tokio::time::{interval, timeout, MissedTickBehavior};

use super::socket::{close_reason, close_session, failure, send};
use super::state::ProductRouteState;
use super::wire::ready_frame;
use nessa_protocol::product_contract::generated::SessionCloseReason;

/// Every method the app dispatcher routes. Which of them a given app may
/// call is Cedar's (`appCredential.call`, policy `app_call.*`), from the
/// operations its owner approved; any other method is `forbidden`.
pub(super) const APP_METHODS: [&str; 10] = [
    "conversation.list",
    "conversation.read",
    "conversation.readEarlier",
    "conversation.receipt",
    "conversation.messageStatus",
    "conversation.create",
    "conversation.observe",
    "conversation.stop",
    "conversation.send",
    "conversation.steer",
];

/// The app session `evidence` proves, when this gateway issues app
/// credentials at all (`None` when it does not).
pub(super) async fn authenticate(
    state: &ProductRouteState,
    evidence: &CredentialEvidence,
) -> Option<Result<ScopedAppSession, AccessError>> {
    let credentials = state.accounts.apps.as_ref()?.credentials()?.clone();
    Some(
        AuthenticateAppSession {
            credentials: credentials.sessions(),
            clock: state.clock.as_ref(),
            policy: state.policy.as_ref(),
        }
        .execute(evidence, &state.audience)
        .await,
    )
}

/// The app's binding as it stands now, or why its session must end.
async fn current(
    state: &ProductRouteState,
    session: &ScopedAppSession,
) -> Result<AppCredentialBinding, AccessError> {
    let Some(credentials) = state
        .accounts
        .apps
        .as_ref()
        .and_then(|apps| apps.credentials())
        .cloned()
    else {
        return Err(AccessError::CredentialRevoked);
    };
    timeout(
        state.settings.handshake_timeout(),
        ReadCurrentAppSession {
            credentials: credentials.sessions(),
            clock: state.clock.as_ref(),
        }
        .execute(session),
    )
    .await
    .map_err(|_| AccessError::Unavailable)?
}

/// Whether the app may call `method`, asking for `asks`, now: `Ok(true)`
/// admitted, `Ok(false)` refused by policy, `Err` when the session must end.
async fn admit(
    state: &ProductRouteState,
    session: &ScopedAppSession,
    method: &str,
    asks: &[AppCallAsk],
) -> Result<bool, AccessError> {
    let Some(credentials) = state
        .accounts
        .apps
        .as_ref()
        .and_then(|apps| apps.credentials())
        .cloned()
    else {
        return Err(AccessError::CredentialRevoked);
    };
    let decided = timeout(
        state.settings.handshake_timeout(),
        AdmitAppCall {
            credentials: credentials.sessions(),
            clock: state.clock.as_ref(),
            policy: state.policy.as_ref(),
        }
        .execute(session, method, asks),
    )
    .await
    .map_err(|_| AccessError::Unavailable)?;
    match decided {
        Ok(_) => Ok(true),
        Err(AccessError::Denied) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Serve one app session, one request at a time, until the app goes, its
/// credential is revoked or expires, or the gateway closes.
pub(super) async fn serve<S>(
    mut socket: S,
    state: ProductRouteState,
    session: ScopedAppSession,
    request_id: String,
    client: String,
) where
    S: Stream<Item = Result<Message, Error>> + Sink<Message> + Unpin + Send + 'static,
{
    let write_timeout = state.settings.write_timeout();
    // The methods it is offered are the ones policy admits now.
    let mut methods = Vec::new();
    for method in APP_METHODS {
        match admit(&state, &session, method, &[]).await {
            Ok(true) => methods.push(method.to_owned()),
            Ok(false) => {}
            Err(error) => {
                close_session(write_timeout, &mut socket, close_reason(error)).await;
                return;
            }
        }
    }
    let ready = ready_frame(
        state.gateway_id().as_str(),
        session.app_identity(),
        Vec::new(),
        methods,
    );
    let Ok(frame) = ResponseFrame::success(&request_id, &ready) else {
        return;
    };
    if send(write_timeout, &mut socket, OutgoingMessage::Response(frame))
        .await
        .is_err()
    {
        return;
    }
    tracing::info!(
        app = session.binding().app().as_str(),
        client = client.as_str(),
        "authenticated app session"
    );
    let mut recheck = interval(state.settings.current_state_interval());
    recheck.set_missed_tick_behavior(MissedTickBehavior::Delay);
    recheck.tick().await;
    let expiry = async {
        match session.expires_at() {
            Some(at) => loop {
                let remaining = at.saturating_sub(state.clock.unix_seconds());
                if remaining == 0 {
                    break;
                }
                crate::timer::clock::sleep(Duration::from_secs(remaining.min(3600))).await;
            },
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(expiry);
    let close = loop {
        let text = tokio::select! {
            _ = state.closed() => break Some(SessionCloseReason::TemporaryUnavailable),
            _ = &mut expiry => break Some(SessionCloseReason::CredentialExpired),
            _ = recheck.tick() => {
                if let Err(error) = current(&state, &session).await {
                    break Some(close_reason(error));
                }
                continue;
            }
            message = socket.next() => match message {
                Some(Ok(Message::Text(text))) => text,
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break None,
                Some(Ok(_)) => continue,
            },
        };
        if text.len() > MAX_PAYLOAD_BYTES as usize {
            break None;
        }
        let Ok(mut frame) = RequestFrame::decode(&text) else {
            continue;
        };
        // Every request asks the credential store and policy again: a
        // revoked app is closed here, before anything it asked is done.
        if frame.kind != "req" || frame.id.is_empty() || frame.id.len() > 256 {
            let response = failure(&frame.id, "invalid_request");
            if send(write_timeout, &mut socket, response).await.is_err() {
                break None;
            }
            continue;
        }
        let response = if !APP_METHODS.contains(&frame.method.as_str()) {
            failure(&frame.id, "forbidden")
        } else {
            match asks(&mut frame) {
                Err(code) => failure(&frame.id, code),
                Ok(asks) => match admit(&state, &session, &frame.method, &asks).await {
                    Ok(true) => answer(&state, &session, frame, &client).await,
                    Ok(false) => failure(&frame.id, "forbidden"),
                    Err(error) => break Some(close_reason(error)),
                },
            }
        };
        if send(write_timeout, &mut socket, response).await.is_err() {
            break None;
        }
    };
    if let Some(reason) = close {
        close_session(write_timeout, &mut socket, reason).await;
    }
}

/// One admitted request's answer, as the app's own principal.
async fn answer(
    state: &ProductRouteState,
    session: &ScopedAppSession,
    frame: RequestFrame,
    client: &str,
) -> OutgoingMessage {
    super::conversation::dispatch_from(
        state,
        session.app_identity(),
        frame,
        Initiation::owner(),
        Some(client),
    )
    .await
}

/// What `frame` asks for beyond its method, for policy to decide
/// (`app_call.shape`), or `invalid_request` when its params do not decode.
/// A new conversation that names no approval mode is given `auto`, never
/// the gateway's default: an app cannot answer a review.
fn asks(frame: &mut RequestFrame) -> Result<Vec<AppCallAsk>, &'static str> {
    let mut asks = Vec::new();
    match frame.method.as_str() {
        "conversation.create" => {
            let Ok(params) =
                serde_json::from_value::<ConversationCreateParams>(frame.params.clone())
            else {
                return Err("invalid_request");
            };
            if params.channel_id.is_some() {
                asks.push(AppCallAsk::Channel);
            }
            if params.environment.is_some() {
                asks.push(AppCallAsk::Environment);
            }
            match params.approval_mode {
                Some(ApprovalMode::Full) => asks.push(AppCallAsk::FullAccess),
                Some(_) => {}
                None => {
                    if let Value::Object(fields) = &mut frame.params {
                        fields.insert(
                            "approvalMode".into(),
                            Value::String(ApprovalMode::Auto.as_str().into()),
                        );
                    }
                }
            }
        }
        "conversation.send" | "conversation.steer" => {
            let links = |key: &str| {
                frame
                    .params
                    .get(key)
                    .and_then(Value::as_array)
                    .is_some_and(|linked| !linked.is_empty())
            };
            if links("files") || links("uploads") {
                asks.push(AppCallAsk::LinkedFiles);
            }
        }
        _ => {}
    }
    Ok(asks)
}

#[cfg(test)]
#[path = "../../tests/product/app_session.rs"]
mod tests;
