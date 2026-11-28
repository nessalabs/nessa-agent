//! An installed app's own session on the Product socket.
//!
//! An app signs in with the credential the owner issued it
//! (`apps.issueCredential`), on the same `/session` route and handshake as
//! any client, so `@nessa/client` works unchanged. What it may then do is
//! decided here and nowhere else:
//!
//! ```text
//! request ──▶ credential still current? (ReadCurrentAppSession) ── no ──▶ close
//!         ──▶ method one of its capabilities'? ─────────────────── no ──▶ forbidden
//!         ──▶ shape an app may ask for? ───────────────────────── no ──▶ forbidden
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
use nessa_auth::application::app_credential::{AppCapability, AppCredentialBinding};
use nessa_auth::application::ports::{AccessError, CredentialEvidence};
use nessa_auth::application::session::{
    AuthenticateAppSession, ReadCurrentAppSession, ScopedAppSession,
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

/// Every method an app may call, and the capability that admits it. Any
/// other method is `forbidden`, whatever the credential holds.
pub(super) const APP_METHODS: [(&str, AppCapability); 10] = [
    ("conversation.list", AppCapability::List),
    ("conversation.read", AppCapability::Read),
    ("conversation.readEarlier", AppCapability::Read),
    ("conversation.receipt", AppCapability::Read),
    ("conversation.messageStatus", AppCapability::Read),
    ("conversation.create", AppCapability::Create),
    ("conversation.observe", AppCapability::Subscribe),
    ("conversation.stop", AppCapability::Stop),
    ("conversation.send", AppCapability::Send),
    ("conversation.steer", AppCapability::Send),
];

/// The capability `method` needs, when an app may call it at all.
pub(super) fn capability_for(method: &str) -> Option<AppCapability> {
    APP_METHODS
        .iter()
        .find(|(name, _)| *name == method)
        .map(|(_, capability)| *capability)
}

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
    let binding = match current(&state, &session).await {
        Ok(binding) => binding,
        Err(error) => {
            close_session(write_timeout, &mut socket, close_reason(error)).await;
            return;
        }
    };
    let methods = binding
        .capabilities()
        .iter()
        .flat_map(|held| {
            APP_METHODS
                .iter()
                .filter(move |(_, needs)| *needs == held)
                .map(|(method, _)| (*method).to_owned())
        })
        .collect();
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
        let Ok(frame) = RequestFrame::decode(&text) else {
            continue;
        };
        // Every request asks the credential store again: a revoked app is
        // closed here, before anything it asked is done.
        let binding = match current(&state, &session).await {
            Ok(binding) => binding,
            Err(error) => break Some(close_reason(error)),
        };
        let response = answer(&state, &session, &binding, frame, &client).await;
        if send(write_timeout, &mut socket, response).await.is_err() {
            break None;
        }
    };
    if let Some(reason) = close {
        close_session(write_timeout, &mut socket, reason).await;
    }
}

/// One request's answer, as the app's own principal.
async fn answer(
    state: &ProductRouteState,
    session: &ScopedAppSession,
    binding: &AppCredentialBinding,
    mut frame: RequestFrame,
    client: &str,
) -> OutgoingMessage {
    if frame.kind != "req" || frame.id.is_empty() || frame.id.len() > 256 {
        return failure(&frame.id, "invalid_request");
    }
    let Some(needs) = capability_for(&frame.method) else {
        return failure(&frame.id, "forbidden");
    };
    if !binding.capabilities().iter().any(|held| held == needs) {
        return failure(&frame.id, "forbidden");
    }
    if let Err(code) = admit_shape(&mut frame) {
        return failure(&frame.id, code);
    }
    super::conversation::dispatch_from(
        state,
        session.app_identity(),
        frame,
        Initiation::owner(),
        Some(client),
    )
    .await
}

/// Refuse what an app may not ask even of its own conversations, and fill
/// in what it may not choose. A new conversation runs in `auto`: an app
/// cannot answer a permission review, and `full` would let it run any
/// tool unasked. It names no channel or environment of the owner's. A
/// message links no file on this computer.
fn admit_shape(frame: &mut RequestFrame) -> Result<(), &'static str> {
    match frame.method.as_str() {
        "conversation.create" => {
            let Ok(params) =
                serde_json::from_value::<ConversationCreateParams>(frame.params.clone())
            else {
                return Err("invalid_request");
            };
            if params.channel_id.is_some() || params.environment.is_some() {
                return Err("forbidden");
            }
            match params.approval_mode {
                Some(ApprovalMode::Full) => Err("forbidden"),
                Some(_) => Ok(()),
                None => {
                    if let Value::Object(fields) = &mut frame.params {
                        fields.insert(
                            "approvalMode".into(),
                            Value::String(ApprovalMode::Auto.as_str().into()),
                        );
                    }
                    Ok(())
                }
            }
        }
        "conversation.send" | "conversation.steer" => {
            let links_files = frame
                .params
                .get("files")
                .and_then(Value::as_array)
                .is_some_and(|files| !files.is_empty())
                || frame
                    .params
                    .get("uploads")
                    .and_then(Value::as_array)
                    .is_some_and(|uploads| !uploads.is_empty());
            if links_files {
                Err("forbidden")
            } else {
                Ok(())
            }
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
#[path = "../../tests/product/app_session.rs"]
mod tests;
