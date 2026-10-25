use super::event_sequence::EventSequence;
use super::passive_read::deadlines::{PASSIVE_READ_TIMEOUT, RECORD_SEND_TIMEOUT};
use super::read_access::ReviewNoticeVisibility;
use super::record_read::refusal_code;
use super::state::{note_limit, ProductRouteState};
use super::subscription::{ConnectionSubscriptions, SubscriptionDeliveries, SubscriptionFrame};
use super::wire::ready_frame;
use crate::browser_session::application::{
    invalidation_reason, BrowserSessionVerifier, ReadBrowserSession,
};
use crate::browser_session::domain::value_objects::RemovalReason;
use crate::conversation::application::{
    access_refusal, AccessDecision, RecordReadLease, ReviewNoticeAuditFailure, WindowSlot,
};
#[cfg(test)]
use crate::conversation_test_support as conversation_support;
use axum::extract::ws::{CloseFrame, Message};
use axum::Error;
use futures_util::stream::{FuturesUnordered, SplitSink};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use nessa_auth::application::authorization::AuthorizeAction;
use nessa_auth::application::credential_admin::{
    CredentialAdminError, IssueCredentialOutcome, IssueCredentialRequest, ListCredentialsRequest,
    RevokeCredentialOutcome, RevokeCredentialRequest,
};
use nessa_auth::application::dto::{CredentialGrantDto, MembershipStateDto, ResourceDto};
use nessa_auth::application::ports::{
    AccessError, AccessSnapshot, CredentialEvidence, CredentialVerifier, Decision, PortFuture,
    SessionEvidence, VerifiedCredential,
};
use nessa_auth::application::session::{
    AuthenticateSession, AuthenticatedSession, ReadCurrentSession, ResumeSession,
};
use nessa_auth::domain::{Action, AudienceId, CredentialId};
#[cfg(test)]
use nessa_protocol::product::generated::wire_shape_product_session_ready;
use nessa_protocol::product::generated::{
    action_for_method, product_event, CredentialIssueParams, CredentialListParams,
    CredentialListResult, CredentialRevokeParams, CredentialRevokeResult, ExistingCredentialResult,
    IssuedCredentialResult, PermissionReviewNotice, PermissionReviewWaiting, ProductSessionReady,
    SessionAuthenticateParams, SessionChallenge, SessionTermination,
    MAX_NATIVE_HOOKS_REQUEST_ID_BYTES, MAX_PRODUCT_CLIENT_ID_CHARACTERS,
    MAX_PRODUCT_SURFACE_INSTANCE_CHARACTERS, MAX_RECORD_RESPONSE_BYTES,
    MIN_NATIVE_HOOKS_REQUEST_ID_CHARACTERS, PRODUCT_HANDSHAKE_METHOD, PRODUCT_READY_METHODS,
    PRODUCT_VERSION,
};
use nessa_protocol::product::handshake::{authentication_close_reason, supports_product_version};
use nessa_protocol::product_contract::generated::SessionCloseReason;
use nessa_protocol::protocol::{
    health_check_message, unique_envelope, EventFrame, OutgoingMessage, RequestFrame,
    ResponseFrame, MAX_PAYLOAD_BYTES,
};
use nessa_sdk::domain::agent_execution::executions::Initiation;
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;
use tokio::sync::mpsc::Receiver;
use tokio::sync::{mpsc, oneshot, watch, OwnedSemaphorePermit, Semaphore};
use tokio::time::error::Elapsed;
use tokio::time::{interval, timeout, timeout_at, Instant, MissedTickBehavior};
use uuid::Uuid;

/// How credential evidence presented on one kind of connection is checked.
/// It is the only thing that differs between the browser socket and a native
/// device connection; authentication itself stays with `AuthenticateSession`
/// and every later admission with this socket (design row PR1).
pub(crate) trait SessionProof<S>: Send + Sync {
    /// The verifier for evidence presented on `socket`. It may borrow what the
    /// socket proved, but holds nothing of the socket across authentication.
    fn verifier<'a>(
        &'a self,
        socket: &'a S,
        state: &'a ProductRouteState,
    ) -> Box<dyn CredentialVerifier + 'a>;
}

/// The browser socket's proof: the opaque bearer secret, checked by the
/// composed `state.verifier`.
struct BearerProof;
impl<S> SessionProof<S> for BearerProof {
    fn verifier<'a>(
        &'a self,
        _: &'a S,
        state: &'a ProductRouteState,
    ) -> Box<dyn CredentialVerifier + 'a> {
        Box::new(Composed(state.verifier.as_ref()))
    }
}
struct Composed<'a>(&'a dyn CredentialVerifier);
impl CredentialVerifier for Composed<'_> {
    fn verify<'a>(
        &'a self,
        evidence: &'a CredentialEvidence,
        audience: &'a AudienceId,
    ) -> PortFuture<'a, VerifiedCredential> {
        self.0.verify(evidence, audience)
    }
}

/// Run one mandatory-authentication product session.
pub async fn handle_socket<S>(socket: S, state: ProductRouteState)
where
    S: Stream<Item = Result<Message, Error>> + Sink<Message> + Unpin + Send + 'static,
{
    serve_session(socket, state, &BearerProof).await
}

/// Run one product session whose credential evidence `proof` checks.
pub(crate) async fn serve_session<S, P>(mut socket: S, state: ProductRouteState, proof: &P)
where
    S: Stream<Item = Result<Message, Error>> + Sink<Message> + Unpin + Send + 'static,
    P: SessionProof<S>,
{
    let deadline = Instant::now() + state.settings.handshake_timeout();
    let nonce = Uuid::new_v4().to_string();
    // Wire timestamps use seconds. Round up from millisecond wall time so the
    // advertisement never truncates the authentication window. Only the shared
    // monotonic deadline below enforces it, including challenge delivery.
    let challenge_expires_at = state
        .clock
        .unix_milliseconds()
        .saturating_add(state.settings.handshake_timeout().as_millis() as u64)
        .div_ceil(1000);
    let challenge = SessionChallenge {
        min_version: PRODUCT_VERSION,
        max_version: PRODUCT_VERSION,
        nonce: nonce.clone(),
        expires_at: challenge_expires_at,
    };
    let challenge =
        match EventFrame::push("session.challenge", &challenge, CHALLENGE_EVENT_SEQUENCE, 0) {
            Ok(frame) => OutgoingMessage::Event(frame),
            Err(_) => return,
        };
    let authenticated = timeout_at(deadline, async {
        send(state.settings.write_timeout(), &mut socket, challenge)
            .await
            .map_err(|_| (String::new(), "temporarily_unavailable"))?;
        receive_authentication(&mut socket, &state, &nonce, deadline, proof).await
    })
    .await;
    let (request_id, session, handshake) = match authenticated {
        Ok(Ok(value)) => value,
        Ok(Err((request_id, code))) => {
            if code != "handshake_timeout" {
                let _ = send_error(
                    state.settings.write_timeout(),
                    &mut socket,
                    &request_id,
                    code,
                )
                .await;
            }
            let reason = authentication_close_reason(code);
            close_session(state.settings.write_timeout(), &mut socket, reason).await;
            return;
        }
        Err(_) => {
            close_session(
                state.settings.write_timeout(),
                &mut socket,
                SessionCloseReason::HandshakeTimeout,
            )
            .await;
            return;
        }
    };

    let (session, snapshot) = match current_identity(&state, &session).await {
        Ok(current) => current,
        Err(error) => {
            close_session(
                state.settings.write_timeout(),
                &mut socket,
                close_reason(error),
            )
            .await;
            return;
        }
    };
    record_authenticated_surface(session.context().principal_id().as_str(), &handshake);
    let ready = session_ready(&state, &session, &snapshot);
    let response = match ResponseFrame::success(&request_id, &ready) {
        Ok(frame) => OutgoingMessage::Response(frame),
        Err(_) => return,
    };
    if send(state.settings.write_timeout(), &mut socket, response)
        .await
        .is_err()
    {
        return;
    }

    run_authenticated(socket, state, session).await;
}

/// Surface and client id a successful handshake named. Neither grants
/// permissions; the credential's principal is the identity.
struct RecordedHandshake {
    client_id: String,
    surface_kind: &'static str,
    surface_instance: String,
}

/// Write the handshake's surface to the gateway log. The callsite is not
/// cached: a test that authenticates before a subscriber exists must not
/// silence this line for the rest of the process.
fn record_authenticated_surface(principal_id: &str, handshake: &RecordedHandshake) {
    static CALLSITE: tracing::callsite::DefaultCallsite =
        tracing::callsite::DefaultCallsite::new(&META);
    static META: tracing::Metadata<'static> = tracing::Metadata::new(
        "authenticated product session",
        "nessa_server::product::socket",
        tracing::Level::INFO,
        Some(file!()),
        Some(line!()),
        Some(module_path!()),
        tracing::field::FieldSet::new(
            &[
                "message",
                "principal_id",
                "client_id",
                "surface_kind",
                "surface_instance",
            ],
            tracing::callsite::Identifier(&CALLSITE),
        ),
        tracing::metadata::Kind::EVENT,
    );

    tracing::dispatcher::get_default(|dispatch| {
        if !dispatch.enabled(&META) {
            return;
        }
        let fields = META.fields();
        let message_field = fields
            .field("message")
            .expect("message is one of the handshake record's fields");
        let principal_field = fields
            .field("principal_id")
            .expect("principal_id is one of the handshake record's fields");
        let client_field = fields
            .field("client_id")
            .expect("client_id is one of the handshake record's fields");
        let kind_field = fields
            .field("surface_kind")
            .expect("surface_kind is one of the handshake record's fields");
        let instance_field = fields
            .field("surface_instance")
            .expect("surface_instance is one of the handshake record's fields");
        let message = "authenticated product session";
        let client_id = handshake.client_id.as_str();
        let surface_kind = handshake.surface_kind;
        let surface_instance = handshake.surface_instance.as_str();
        let values = [
            (&message_field, Some(&message as &dyn tracing::field::Value)),
            (
                &principal_field,
                Some(&principal_id as &dyn tracing::field::Value),
            ),
            (
                &client_field,
                Some(&client_id as &dyn tracing::field::Value),
            ),
            (
                &kind_field,
                Some(&surface_kind as &dyn tracing::field::Value),
            ),
            (
                &instance_field,
                Some(&surface_instance as &dyn tracing::field::Value),
            ),
        ];
        let values = fields.value_set(&values);
        dispatch.event(&tracing::Event::new(&META, &values));
    });
}

/// Refuse an instance the product schema does not admit. Kind is the generated
/// enum, so an unknown kind never reaches here.
fn bounded_surface(params: &SessionAuthenticateParams) -> Result<RecordedHandshake, &'static str> {
    let characters = params.surface.instance.chars().count();
    if characters == 0 {
        return Err("unauthorized");
    }
    if characters > MAX_PRODUCT_SURFACE_INSTANCE_CHARACTERS {
        note_limit("product.max_surface_instance_characters");
        return Err("unauthorized");
    }
    Ok(RecordedHandshake {
        client_id: params.client.id.clone(),
        surface_kind: params.surface.kind.as_str(),
        surface_instance: params.surface.instance.clone(),
    })
}

async fn receive_authentication<S, P>(
    socket: &mut S,
    state: &ProductRouteState,
    nonce: &str,
    deadline: Instant,
    proof: &P,
) -> Result<(String, AuthenticatedSession, RecordedHandshake), (String, &'static str)>
where
    S: Stream<Item = Result<Message, Error>> + Unpin,
    P: SessionProof<S>,
{
    let Some(Ok(Message::Text(text))) = socket.next().await else {
        return Err((String::new(), "unauthorized"));
    };
    if Instant::now() >= deadline {
        return Err((String::new(), "handshake_timeout"));
    }
    if text.len() > MAX_PAYLOAD_BYTES as usize {
        note_limit("product.max_payload_bytes");
        return Err((String::new(), "unauthorized"));
    }
    let frame = RequestFrame::decode(&text).map_err(|_| (String::new(), "unauthorized"))?;
    if frame.kind != "req"
        || frame.id.is_empty()
        || frame.id.len() > 256
        || frame.method != PRODUCT_HANDSHAKE_METHOD
    {
        return Err((frame.id, "authentication_required"));
    }
    let params: SessionAuthenticateParams =
        serde_json::from_value(frame.params).map_err(|_| (frame.id.clone(), "unauthorized"))?;
    if !supports_product_version(params.min_version, params.max_version) {
        return Err((frame.id, "protocol_incompatible"));
    }
    if params.nonce != nonce || params.client.id.is_empty() {
        return Err((frame.id, "unauthorized"));
    }
    if params.client.id.len() > MAX_PRODUCT_CLIENT_ID_CHARACTERS {
        note_limit("product.max_client_id_characters");
        return Err((frame.id, "unauthorized"));
    }
    let handshake = match bounded_surface(&params) {
        Ok(handshake) => handshake,
        Err(code) => return Err((frame.id, code)),
    };
    if let Some(id) = &state.browser_session_id {
        if !params.credential.is_empty() {
            return Err((frame.id, "unauthorized"));
        }
        let store = state
            .browser_sessions
            .as_ref()
            .ok_or_else(|| (frame.id.clone(), "unauthorized"))?;
        let expected_origin = state
            .browser_session_origin
            .as_deref()
            .ok_or_else(|| (frame.id.clone(), "unauthorized"))?;
        let _verified_record = ReadBrowserSession {
            store: store.as_ref(),
        }
        .execute(id, state.clock.unix_seconds())
        .await
        .map_err(|_| (frame.id.clone(), "temporarily_unavailable"))?
        .filter(|session| session.origin() == expected_origin)
        .ok_or_else(|| (frame.id.clone(), "unauthorized"))?;
        let evidence = SessionEvidence::new(id.as_bytes().to_vec())
            .map_err(|_| (frame.id.clone(), "unauthorized"))?;
        let verifier = BrowserSessionVerifier {
            store: store.as_ref(),
            expected_origin,
            now: state.clock.unix_seconds(),
        };
        let identity = (ResumeSession {
            verifier: &verifier,
            access: state.access.as_ref(),
            clock: state.clock.as_ref(),
        })
        .execute(&evidence, &state.audience)
        .await;
        let identity = match identity {
            Ok((identity, _)) => identity,
            Err(error) => {
                if let Some(reason) = invalidation_reason(error) {
                    store
                        .remove(id.clone(), state.clock.unix_seconds(), reason, None)
                        .await
                        .map_err(|_| (frame.id.clone(), "temporarily_unavailable"))?;
                }
                return Err((
                    frame.id,
                    if retryable_access_error(error) {
                        "temporarily_unavailable"
                    } else {
                        "unauthorized"
                    },
                ));
            }
        };
        return Ok((frame.id, identity, handshake));
    }
    let evidence = CredentialEvidence::new(params.credential.into_bytes())
        .map_err(|_| (frame.id.clone(), "unauthorized"))?;
    let verifier = proof.verifier(socket, state);
    let session = AuthenticateSession {
        verifier: verifier.as_ref(),
        access: state.access.as_ref(),
        clock: state.clock.as_ref(),
    }
    .execute(&evidence, &state.audience)
    .await
    .map_err(|error| {
        (
            frame.id.clone(),
            if retryable_access_error(error) {
                "temporarily_unavailable"
            } else {
                "unauthorized"
            },
        )
    })?;
    if Instant::now() >= deadline {
        return Err((frame.id, "handshake_timeout"));
    }
    Ok((frame.id, session, handshake))
}

fn credential_admin_code(error: CredentialAdminError) -> &'static str {
    match error {
        CredentialAdminError::Conflict => "credential_conflict",
        CredentialAdminError::Capacity => "credential_capacity",
        CredentialAdminError::NotFound => "credential_not_found",
        CredentialAdminError::Unavailable => "credential_store_unavailable",
    }
}

/// One record frame in flight on a socket. Fixed: the lane below is this wide.
pub(crate) const RECORD_SLOT: usize = 1;
/// One record response queued on a socket.
pub(crate) const RECORD_LANE: usize = 1;
/// One refusal queued while the writer is inside another frame.
pub(crate) const REFUSAL_LANE: usize = 1;

/// The mount a socket's app call belongs to: the conversation and the host's
/// `McpAppReference`. The lane's cap is per mount
/// (`one_mount_cannot_fill_the_app_lane`).
#[derive(Clone, PartialEq, Eq, Hash)]
struct AppMountKey {
    conversation_id: String,
    execution_id: String,
    tool_id: String,
    instance_id: String,
}

/// One admitted app call's share of its mount's cap. Dropped with the slot it
/// reserved, including when that slot is refused after the reservation
/// (`a_lane_refusal_does_not_stick_to_the_mount`).
struct AppMountGuard {
    key: AppMountKey,
    mounts: Arc<Mutex<HashMap<AppMountKey, usize>>>,
}

impl Drop for AppMountGuard {
    fn drop(&mut self) {
        let mut counts = self
            .mounts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(count) = counts.get_mut(&self.key) else {
            return;
        };
        *count -= 1;
        if *count == 0 {
            counts.remove(&self.key);
        }
    }
}

/// The mount is already at its cap. The call is refused before it takes a
/// lane slot.
struct MountCap;

/// Reserve one of `app`'s mount's `cap` places, when the frame names a mount.
/// A frame that does not is not counted: an empty `mcp.callTool` still meets
/// the socket lane and is named `socket.app_calls`
/// (`a_full_socket_slot_set_names_that_limit`). `cap` is one less than that
/// lane, so a mount cannot take the last slot
/// (`one_mount_cannot_fill_the_app_lane`).
fn reserve_app_mount(
    mounts: &Arc<Mutex<HashMap<AppMountKey, usize>>>,
    frame: &RequestFrame,
    cap: usize,
) -> Result<Option<AppMountGuard>, MountCap> {
    let Some(key) = app_mount_key(&frame.params) else {
        return Ok(None);
    };
    let mut counts = mounts
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let count = counts.get(&key).copied().unwrap_or(0);
    if count >= cap {
        return Err(MountCap);
    }
    counts.insert(key.clone(), count + 1);
    drop(counts);
    Ok(Some(AppMountGuard {
        key,
        mounts: Arc::clone(mounts),
    }))
}

/// The mount `params` names, when it is one [`super::mcp_apps::counted_mount`]
/// accepts: the same conversation and app dispatch would. Anything else is
/// left uncounted, so the method's own schema refuses it and a full lane
/// still names `socket.app_calls`.
fn app_mount_key(params: &serde_json::Value) -> Option<AppMountKey> {
    let mount = super::mcp_apps::counted_mount(params)?;
    Some(AppMountKey {
        conversation_id: mount.conversation_id,
        execution_id: mount.execution_id,
        tool_id: mount.tool_id,
        instance_id: mount.instance_id,
    })
}

/// App calls read while one app refusal waits for the refusal lane. A control
/// frame behind them is still admitted. Past this, reading pauses so the hold
/// cannot grow without bound.
/// `a_release_is_admitted_while_an_app_refusal_waits`.
const HELD_INPUT_WHILE_APP_REFUSAL: usize = 16;

#[derive(Clone, Copy)]
enum ResponseClass {
    Control,
    Ordinary,
    Record,
    /// An MCP App's call (#348): a destructive one waits minutes on the
    /// person's review, so app calls have a lane of their own, and held calls
    /// never take the place of the read and the answer that would end them.
    App,
}

impl ResponseClass {
    fn for_method(method: &str) -> Self {
        match method {
            "conversation.close"
            | "conversation.stop"
            | "conversation.archive"
            | "conversation.unarchive"
            | "conversation.rename"
            | "conversation.answer"
            // A person reads a long review or question before they answer
            // it, so the read is never behind ordinary requests either.
            | "conversation.permissionReview"
            | "conversation.questionReview"
            | "conversation.answerQuestion"
            | "conversation.cancel"
            | "conversation.remove"
            | "conversation.reorder"
            // Releasing an app ends its held calls, so it is never behind
            // them on the app lane.
            | "mcp.releaseApp"
            // Ending an enrollment is never crowded out by ordinary requests
            // (design row O7).
            | "pairing.deny"
            | "pairing.cancel" => Self::Control,
            "mcp.callTool"
            | "mcp.readResource"
            // A message can wait on the person's review, as a call can.
            | "mcp.sendMessage"
            | "mcp.updateModelContext" => Self::App,
            "conversation.recordsHead"
            | "conversation.recordsPage"
            | "conversation.catalogueHead"
            | "conversation.catalogueManifest"
            | "conversation.catalogueResolve" => Self::Record,
            _ => Self::Ordinary,
        }
    }
}

struct QueuedResponse {
    message: WireResponse,
    _slot: Arc<OwnedSemaphorePermit>,
    // Record replies transfer this to QueuedRecordResponse, whose writer
    // releases delivery ownership before sending (R64).
    // The read adapter must not return it until its non-entered source thread
    // has finished and joined the SDK worker, including after caller timeout.
    _record_work: Option<RecordReadLease>,
    // Held until this response is taken to send, with the slot: a mount's
    // cap lasts exactly as long as the lane place it reserved.
    _mount: Option<AppMountGuard>,
}

enum WireResponse {
    Ordinary(Box<OutgoingMessage>),
    Record {
        text: String,
    },
    /// A waiting-review snapshot. Its sequence number is taken when the
    /// writer sends it, so it follows the same order as a subscription frame.
    ReviewWaiting {
        payload: PermissionReviewWaiting,
        generation: u64,
    },
}

impl WireResponse {
    fn ordinary(message: OutgoingMessage) -> Self {
        Self::Ordinary(Box::new(message))
    }
    fn record(text: String) -> Self {
        Self::Record { text }
    }
}

// The lane owns its lifetime, including ordinary wire refusals of passive reads.
struct QueuedRecordResponse {
    message: WireResponse,
    _slot: Option<Arc<OwnedSemaphorePermit>>,
    _record_work: Option<RecordReadLease>,
    _mount: Option<AppMountGuard>,
    deadline: Instant,
}
impl QueuedRecordResponse {
    fn new(response: QueuedResponse) -> Self {
        Self::owned(
            response.message,
            Some(response._slot),
            response._record_work,
            response._mount,
        )
    }

    // A refused admission owns delivery only, never a fabricated read permit.
    fn refusal(message: OutgoingMessage) -> Self {
        Self::owned(WireResponse::ordinary(message), None, None, None)
    }

    fn owned(
        message: WireResponse,
        slot: Option<Arc<OwnedSemaphorePermit>>,
        record_work: Option<RecordReadLease>,
        mount: Option<AppMountGuard>,
    ) -> Self {
        Self {
            message,
            _slot: slot,
            _record_work: record_work,
            _mount: mount,
            deadline: Instant::now() + RECORD_SEND_TIMEOUT,
        }
    }
}

enum ControlOutput {
    Response(Box<QueuedResponse>),
}

enum WriterResponse {
    Record(Box<QueuedRecordResponse>),
    Queued(Box<QueuedResponse>),
    Refusal(Box<OutgoingMessage>),
    Subscription(Box<SubscriptionFrame>),
}

// One argument per lane the writer chooses between.
#[allow(clippy::too_many_arguments)]
async fn write_authenticated<S>(
    mut sink: SplitSink<S, Message>,
    mut controls: Receiver<ControlOutput>,
    // The terminal close has its own one-value slot so a full control lane
    // cannot crowd it out. It is written after the controls already queued.
    close: oneshot::Receiver<SessionCloseReason>,
    mut refusals: Receiver<OutgoingMessage>,
    mut ordinary: Receiver<QueuedResponse>,
    mut records: Receiver<QueuedRecordResponse>,
    write_timeout: Duration,
    sequence: Arc<EventSequence>,
    subscriptions: Arc<SubscriptionDeliveries>,
) where
    S: Stream<Item = Result<Message, Error>> + Sink<Message> + Unpin,
{
    // Moving the record out of its lane observes its original deadline, not a
    // physical-send priority change. The retained-position bound is documented in
    // docs/design/authorized-record-reads.md, R62.
    let mut pending_record: Option<QueuedRecordResponse> = None;
    let mut close = Some(close);
    loop {
        let deadline = retained_delivery_deadline(&pending_record, &subscriptions);
        let next = tokio::select! {
            biased;
            () = wait_for_record_deadline(deadline), if deadline.is_some() => {
                note_elapsed_delivery(&pending_record, &subscriptions);
                break;
            },
            Some(response) = records.recv(), if pending_record.is_none() => {
                pending_record = Some(response);
                continue;
            },
            Some(control) = controls.recv() => match control {
                ControlOutput::Response(response) => Some(Ok(WriterResponse::Queued(response))),
            },
            reason = async { close.as_mut().expect("close selected while present").await }, if close.is_some() => {
                close = None;
                match reason {
                    Ok(reason) => Some(Err(reason)),
                    // The owner ended without a reason: drain what is queued.
                    Err(_) => continue,
                }
            },
            Some(response) = refusals.recv() => Some(Ok(WriterResponse::Refusal(Box::new(response)))),
            Some(response) = ordinary.recv() => Some(Ok(WriterResponse::Queued(Box::new(response)))),
            () = std::future::ready(()), if pending_record.is_some() =>
                Some(Ok(WriterResponse::Record(Box::new(pending_record.take().expect("pending record selected"))))),
            // Last: a subscription frame is a replacement that waits its turn;
            // the oldest offer goes first, so no subscription starves another.
            Some(frame) = async { subscriptions.take() }, if !subscriptions.is_closed() =>
                Some(Ok(WriterResponse::Subscription(Box::new(frame)))),
            () = subscriptions.changed(), if !subscriptions.is_closed() => continue,
            else => None,
        };
        let writing = async {
            match next {
                Some(Ok(WriterResponse::Record(response))) => {
                    send_record_queued(write_timeout, &mut sink, *response)
                        .await
                        .is_ok()
                }
                Some(Ok(WriterResponse::Queued(response))) => {
                    let QueuedResponse {
                        message,
                        _slot,
                        _record_work,
                        _mount,
                    } = *response;
                    let Some(message) = number_review_notice(message, &sequence) else {
                        return true;
                    };
                    send_queued(write_timeout, &mut sink, message).await.is_ok()
                }
                Some(Ok(WriterResponse::Subscription(frame))) => {
                    let SubscriptionFrame {
                        id,
                        message,
                        deadline,
                        written,
                    } = *frame;
                    let result = match deadline.at() {
                        Some(deadline) => {
                            within_deadline(deadline, send(write_timeout, &mut sink, message)).await
                        }
                        None => Some(send(write_timeout, &mut sink, message).await),
                    };
                    if matches!(result, Some(Ok(()))) {
                        subscriptions.sent(&id, deadline.last());
                        written.told();
                        true
                    } else {
                        false
                    }
                }
                Some(Ok(WriterResponse::Refusal(message))) => {
                    send(write_timeout, &mut sink, *message).await.is_ok()
                }
                Some(Err(reason)) => {
                    close_session(write_timeout, &mut sink, reason).await;
                    false
                }
                None => false,
            }
        };
        tokio::pin!(writing);
        loop {
            let deadline = retained_delivery_deadline(&pending_record, &subscriptions);
            tokio::select! {
                biased;
                // Abandon this sink; a second frame must not follow a cancelled
                // physical write. Also observe a record arriving during it.
                () = wait_for_record_deadline(deadline), if deadline.is_some() => {
                    note_elapsed_delivery(&pending_record, &subscriptions);
                    return;
                },
                Some(response) = records.recv(), if pending_record.is_none() =>
                    pending_record = Some(response),
                () = subscriptions.changed(), if !subscriptions.is_closed() => {},
                succeeded = &mut writing => {
                    if !succeeded { return; }
                    break;
                },
            }
        }
    }
}

/// The socket's event sequence: `session.challenge` is its first event, and
/// every event after authentication (subscription frames and review notices,
/// from the connection's [`EventSequence`]) continues from here, so one socket
/// never repeats a sequence number.
pub(super) const CHALLENGE_EVENT_SEQUENCE: u64 = 1;

fn retained_delivery_deadline(
    record: &Option<QueuedRecordResponse>,
    subscriptions: &SubscriptionDeliveries,
) -> Option<Instant> {
    [queued_record_deadline(record), subscriptions.deadline()]
        .into_iter()
        .flatten()
        .min()
}

fn queued_record_deadline(response: &Option<QueuedRecordResponse>) -> Option<Instant> {
    response.as_ref().map(|response| response.deadline)
}

async fn wait_for_record_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

// Tokio polls the inner future before its timer. Gate each continuation before
// it can admit source work or emit a frame, then reject completion at expiry.
async fn within_deadline<F: Future>(deadline: Instant, work: F) -> Option<F::Output> {
    tokio::pin!(work);
    let guarded = poll_fn(|context| {
        if Instant::now() >= deadline {
            Poll::Ready(None)
        } else {
            work.as_mut().poll(context).map(Some)
        }
    });
    timeout_at(deadline, guarded)
        .await
        .ok()
        .flatten()
        .filter(|_| Instant::now() < deadline)
}

fn missed_record_deadline<T>(value: Option<T>) -> Option<T> {
    if value.is_none() {
        note_limit("socket.record_delivery_deadline");
    }
    value
}

fn note_elapsed_delivery(
    record: &Option<QueuedRecordResponse>,
    subscriptions: &SubscriptionDeliveries,
) {
    let now = Instant::now();
    if queued_record_deadline(record).is_some_and(|deadline| now >= deadline) {
        note_limit("socket.record_delivery_deadline");
    }
    if subscriptions
        .deadline()
        .is_some_and(|deadline| now >= deadline)
    {
        note_limit("socket.subscription_delivery_deadline");
    }
}

fn rejected_lane<T>(
    limit: &'static str,
    result: Result<(), tokio::sync::mpsc::error::TrySendError<T>>,
) -> bool {
    match result {
        Ok(()) => false,
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            note_limit(limit);
            true
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => true,
    }
}

/// Queue a refusal, or park it in `waiting` when the lane is full rather than
/// closing the socket: a client that asks for more than its slots is told so,
/// frame by frame. While one waits the socket reads no further frame, and
/// still polls its writer, expiry, shutdown and finished requests; the loop
/// sends it once the writer takes the one ahead
/// (`a_waiting_refusal_does_not_stop_the_socket_closing`). Answers whether
/// the socket must close: the writer is gone, or, never on a path the loop
/// takes, a second refusal found the place taken.
fn refuse_or_wait(
    lane: &tokio::sync::mpsc::Sender<OutgoingMessage>,
    waiting: &mut Option<OutgoingMessage>,
    refusal: OutgoingMessage,
) -> bool {
    match lane.try_send(refusal) {
        Ok(()) => false,
        Err(tokio::sync::mpsc::error::TrySendError::Full(refusal)) => {
            note_limit("socket.refusal_lane");
            waiting.replace(refusal).is_some()
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => true,
    }
}

enum AuthenticatedInput {
    Request(RequestFrame),
    Refusal(OutgoingMessage),
}

type AuthorityCheck<'a> = Pin<Box<dyn Future<Output = Option<AccessError>> + Send + 'a>>;
type RefreshCheck<'a> = Pin<
    Box<
        dyn Future<Output = Result<(AuthenticatedSession, AccessSnapshot), AccessError>>
            + Send
            + 'a,
    >,
>;

async fn run_authenticated<S>(socket: S, state: ProductRouteState, session: AuthenticatedSession)
where
    S: Stream<Item = Result<Message, Error>> + Sink<Message> + Unpin + Send + 'static,
{
    let (sink, mut incoming) = socket.split();
    // The socket's one event numbering, continued by its subscription frames
    // and its review notices in the order the writer sends them.
    let sequence = Arc::new(EventSequence::default());
    let mut subscriptions = ConnectionSubscriptions::new(sequence.clone());
    let limits = state.limits;
    let (control_send, control_receive) = mpsc::channel(limits.control_lane());
    let (close_send, close_receive) = oneshot::channel();
    // Why this owner ended the session, if it chose to. Sent once, after the
    // loop, on its own slot (#979).
    let mut close: Option<SessionCloseReason> = None;
    let (refusal_send, refusal_receive) = mpsc::channel(REFUSAL_LANE);
    // Room for every ordinary slot's response and every app call's, which
    // share it: a full queue closes the socket.
    let (ordinary_send, ordinary_receive) = mpsc::channel(limits.ordinary_lane());
    let (record_send, record_receive) = mpsc::channel(RECORD_LANE);
    let mut writer = tokio::spawn(write_authenticated(
        sink,
        control_receive,
        close_receive,
        refusal_receive,
        ordinary_receive,
        record_receive,
        state.settings.write_timeout(),
        sequence,
        subscriptions.deliveries.clone(),
    ));
    let control_slots = Arc::new(Semaphore::new(limits.control_slots()));
    let ordinary_slots = Arc::new(Semaphore::new(limits.ordinary_slots()));
    // A registry revision is global, but each refresh wakes only this socket's
    // publisher. Share changes keep their shared visibility notification.
    let (review_authority, authority_updates) = watch::channel(session.auth_revision());
    // A waiting review is a snapshot on the ordinary lane. Abort this before
    // the lane's sender is dropped: the task holds one, and the writer
    // finishes only when every sender is gone.
    let mut notice_task = state.permission_rules.clone().map(|board| {
        tokio::spawn(forward_review_notices(
            board,
            ordinary_send.clone(),
            Arc::clone(&ordinary_slots),
            state.clone(),
            session.clone(),
            authority_updates,
        ))
    });
    // This connection's place for its window (`workspace.publish`), withdrawn
    // when the socket goes and this is dropped.
    let window = Arc::new(state.conversations.as_ref().map(|conversations| {
        conversations.workspace_windows().open(
            session.context().organization_id().clone(),
            session.context().principal_id().clone(),
        )
    }));
    let record_slots = Arc::new(Semaphore::new(RECORD_SLOT));
    let app_slots = Arc::new(Semaphore::new(limits.app_calls_per_socket()));
    let app_mounts = Arc::new(Mutex::new(HashMap::new()));
    // An app call still running when the socket goes is cancelled: its
    // review, if it is waiting on one, is withdrawn rather than left standing
    // for nobody. One already sent finishes, and is recorded, on its own task.
    let mut app_calls: Vec<tokio::task::AbortHandle> = Vec::new();
    // One app-lane refusal waiting for the single place on the refusal lane.
    // A release answer is control, and the writer sends control before a
    // refusal, so a second call past the lane used to find that place full
    // and close the socket.
    // `a_burst_of_reads_and_releases_refuses_the_rest_and_keeps_the_socket`.
    let mut app_refusal: Option<OutgoingMessage> = None;
    // App calls read while that refusal waits. A control frame is admitted
    // ahead of them, so a stalled refusal cannot hold a release unread.
    // `a_release_is_admitted_while_an_app_refusal_waits`.
    let mut held_input: VecDeque<RequestFrame> = VecDeque::new();
    // Any other refusal waiting for the refusal lane (`refuse_or_wait`). No
    // frame is read while it waits, so it never needs a second place.
    let mut waiting_refusal: Option<OutgoingMessage> = None;
    let mut current_state = interval(state.settings.current_state_interval());
    current_state.set_missed_tick_behavior(MissedTickBehavior::Delay);
    current_state.tick().await;
    let expiry = async {
        if let Some(expires_at) = session.expires_at() {
            loop {
                let remaining = expires_at.saturating_sub(state.clock.unix_seconds());
                if remaining == 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(remaining.min(3600))).await;
            }
        } else {
            std::future::pending::<()>().await;
        }
    };
    tokio::pin!(expiry);
    // Admission retains a response slot through the physical write (a record's
    // until the writer takes it to send, row R64), so a stalled sink
    // does not stop this owner from receiving or dispatching controls.
    let mut requests = FuturesUnordered::new();
    let mut writer_finished = false;
    let mut refresh: Option<RefreshCheck<'_>> = None;
    let mut input_check: Option<AuthorityCheck<'_>> = None;
    let mut pending_input: Option<AuthenticatedInput> = None;
    loop {
        // A held app call is admitted only once the waiting refusal has a
        // place. Until then the socket still reads, and a control frame is
        // admitted ahead of what is held.
        let staged_record = if app_refusal.is_none()
            && waiting_refusal.is_none()
            && pending_input.is_none()
            && input_check.is_none()
        {
            held_input.pop_front().and_then(|frame| {
                if matches!(
                    ResponseClass::for_method(&frame.method),
                    ResponseClass::Record
                ) {
                    Some(frame)
                } else {
                    pending_input = Some(AuthenticatedInput::Request(frame));
                    input_check = Some(Box::pin(current_session_error(&state, &session)));
                    None
                }
            })
        } else {
            None
        };
        // Authority checks suspend only their own admission. Request completion,
        // delivery teardown and expiry retain independently polled owners (R60).
        let admitted = if let Some(frame) = staged_record {
            Some((frame, Instant::now()))
        } else {
            tokio::select! {
            _ = &mut writer => {
                writer_finished = true;
                break;
            }
            outcome = async { notice_task.as_mut().expect("notice task selected while present").await }, if notice_task.is_some() => {
                // Fatal publication belongs to this connection owner, and it
                // ends the connection whatever the control lane holds.
                notice_task = None;
                let reason = match outcome {
                    Ok(Err(failure)) => {
                        tracing::error!(?failure, "review snapshot publication failed");
                        failure.close_reason()
                    }
                    Ok(Ok(())) | Err(_) => {
                        tracing::error!("review snapshot publisher ended unexpectedly");
                        SessionCloseReason::TemporaryUnavailable
                    }
                };
                close = Some(reason);
                break;
            }
            _ = state.closed() => {
                close = Some(SessionCloseReason::TemporaryUnavailable);
                break;
            }
            Some(result) = requests.next(), if !requests.is_empty() => {
                let Ok((message, class, slot, record_work, mount)) = result else { break };
                let queued = QueuedResponse {
                    message,
                    _slot: slot,
                    _record_work: record_work,
                    _mount: mount,
                };
                let rejected = match class {
                    ResponseClass::Control => rejected_lane(
                        "socket.control_lane",
                        control_send.try_send(ControlOutput::Response(Box::new(queued))),
                    ),
                    ResponseClass::Ordinary | ResponseClass::App => rejected_lane(
                        "socket.ordinary_lane",
                        ordinary_send.try_send(queued),
                    ),
                    ResponseClass::Record => rejected_lane(
                        "socket.record_lane",
                        record_send.try_send(QueuedRecordResponse::new(queued)),
                    ),
                };
                if rejected {
                    break;
                }
                None
            }
            _ = &mut expiry => {
                close = Some(SessionCloseReason::CredentialExpired);
                break;
            }
            current = async { refresh.as_mut().expect("refresh selected while pending").await }, if refresh.is_some() => {
                refresh = None;
                match current {
                    Ok((_, snapshot)) => {
                        review_authority.send_if_modified(|revision| {
                            if *revision != snapshot.revision {
                                *revision = snapshot.revision;
                                true
                            } else {
                                false
                            }
                        });
                    }
                    Err(error) => {
                        close = Some(close_reason(error));
                        break;
                    }
                }
                None
            }
            _ = current_state.tick(), if refresh.is_none() => {
                refresh = Some(Box::pin(current_identity(&state, &session)));
                None
            }
            permit = refusal_send.reserve(), if app_refusal.is_some() => {
                match permit {
                    Ok(permit) => {
                        permit.send(
                            app_refusal
                                .take()
                                .expect("refusal reserve is gated on a waiting app call"),
                        );
                        None
                    }
                    // The writer has dropped the lane.
                    Err(_) => break,
                }
            }
            permit = refusal_send.reserve(), if waiting_refusal.is_some() => {
                match permit {
                    Ok(permit) => {
                        permit.send(
                            waiting_refusal
                                .take()
                                .expect("refusal reserve is gated on a waiting refusal"),
                        );
                        None
                    }
                    // The writer has dropped the lane.
                    Err(_) => break,
                }
            }
            error = async { input_check.as_mut().expect("input check selected while pending").await }, if input_check.is_some() => {
                input_check = None;
                if let Some(error) = error {
                    close = Some(close_reason(error));
                    break;
                }
                match pending_input.take().expect("input check owns one input") {
                    AuthenticatedInput::Request(frame) => Some((frame, Instant::now())),
                    AuthenticatedInput::Refusal(response) => {
                        if refuse_or_wait(&refusal_send, &mut waiting_refusal, response) {
                            break;
                        }
                        None
                    }
                }
            }
            message = incoming.next(), if pending_input.is_none()
                && waiting_refusal.is_none()
                && held_input.len() < HELD_INPUT_WHILE_APP_REFUSAL
                && (app_refusal.is_some() || held_input.is_empty()) => {
                let Some(Ok(message)) = message else { break };
                let Message::Text(text) = message else {
                    if matches!(message, Message::Close(_)) { break; }
                    continue;
                };
                if text.len() > MAX_PAYLOAD_BYTES as usize {
                    note_limit("product.max_payload_bytes");
                    break;
                }
                let frame: RequestFrame = match RequestFrame::decode(&text) {
                    Ok(frame) => frame,
                    Err(_) => {
                        let Some(response) = correlatable_invalid_request(&text) else { continue };
                        pending_input = Some(AuthenticatedInput::Refusal(response));
                        input_check = Some(Box::pin(current_session_error(&state, &session)));
                        continue;
                    },
                };
                // A further app call would need another refusal place. Hold it
                // and keep reading so a release behind it is still admitted.
                if app_refusal.is_some()
                    && matches!(ResponseClass::for_method(&frame.method), ResponseClass::App)
                {
                    held_input.push_back(frame);
                    continue;
                }
                let record = matches!(ResponseClass::for_method(&frame.method), ResponseClass::Record);
                if record {
                    Some((frame, Instant::now()))
                } else {
                    // Every deferred input asks current authority independently;
                    // periodic refresh cannot authorize an input. Later inputs
                    // wait behind this one without accumulating another queue.
                    pending_input = Some(AuthenticatedInput::Request(frame));
                    input_check = Some(Box::pin(current_session_error(&state, &session)));
                    None
                }
            }
            }
        };
        let Some((frame, received_at)) = admitted else {
            continue;
        };
        let read_deadline = received_at + PASSIVE_READ_TIMEOUT;
        let class = ResponseClass::for_method(&frame.method);
        let control = matches!(class, ResponseClass::Control);
        let record = matches!(class, ResponseClass::Record);
        let app = matches!(class, ResponseClass::App);
        // Before the lane's own slot: a mount at its cap is refused without
        // taking one, so the last slot stays available to another mount
        // (`one_mount_cannot_fill_the_app_lane`).
        let mount_guard = if app {
            match reserve_app_mount(&app_mounts, &frame, limits.app_calls_per_mount()) {
                Ok(guard) => guard,
                Err(MountCap) => {
                    // The refusal lane holds one frame. Parking this refusal,
                    // as a full lane does, keeps a second capped call from
                    // closing the socket while the writer is inside a control
                    // frame (`a_mount_cap_refusal_waits_and_keeps_the_socket`).
                    note_limit("socket.app_mount");
                    app_refusal = Some(failure(&frame.id, "temporarily_unavailable"));
                    continue;
                }
            }
        } else {
            None
        };
        let slots = if control {
            &control_slots
        } else if record {
            &record_slots
        } else if app {
            &app_slots
        } else {
            &ordinary_slots
        };
        let Ok(slot) = slots.clone().try_acquire_owned() else {
            // The reservation must not outlive the slot it did not get
            // (`a_lane_refusal_does_not_stick_to_the_mount`).
            drop(mount_guard);
            note_limit(match class {
                ResponseClass::Control => "socket.control_slots",
                ResponseClass::Record => "socket.record_slot",
                ResponseClass::App => "socket.app_calls",
                ResponseClass::Ordinary => "socket.ordinary_slots",
            });
            let response = failure(&frame.id, "temporarily_unavailable");
            if app {
                app_refusal = Some(response);
                continue;
            }
            let refused = if record {
                rejected_lane(
                    "socket.record_lane",
                    record_send.try_send(QueuedRecordResponse::refusal(response)),
                )
            } else {
                refuse_or_wait(&refusal_send, &mut waiting_refusal, response)
            };
            if refused {
                break;
            }
            continue;
        };
        let slot = Arc::new(slot);
        if ConnectionSubscriptions::method(&frame.method) {
            // Admitted as any request is (row S29): an unsubscribe here,
            // before it changes anything; a subscribe in its own task,
            // before it registers anything. The unsubscribe is admitted
            // inline on purpose: its stop lands before the next frame is
            // read, so a subscribe to the same target sent after it is begun
            // only once the old one is gone. Moved into a task, a client's
            // close-then-subscribe would race and be refused as a duplicate.
            let refused = if ConnectionSubscriptions::stops(&frame.method) {
                admit_now(&state, &session, &frame.method)
                    .await
                    .err()
                    .map(|code| failure(&frame.id, code))
            } else {
                None
            };
            let method = frame.method.clone();
            let reply = match refused {
                Some(refused) => Some(refused),
                None => subscriptions.begin(&state, &session, frame, slot.clone(), received_at),
            };
            if let Some(reply) = reply {
                super::conversation::trace_refused_command(&method, &reply);
                let queued = QueuedResponse {
                    message: WireResponse::ordinary(reply),
                    _slot: slot,
                    _record_work: None,
                    _mount: None,
                };
                if rejected_lane("socket.ordinary_lane", ordinary_send.try_send(queued)) {
                    break;
                }
            }
            continue;
        }
        // An app call's capacity across sockets is the conversation
        // service's, held by the call's own task until it ends: a permit held
        // here would be let go when the socket went, while the call ran on.
        let (capacity, limit_id) = if control {
            (Some(&state.controls), "server.controls")
        } else if record {
            (Some(&state.record_reads), "server.record_reads")
        } else if app {
            (None, "socket.app_calls")
        } else if frame.method == "conversation.delete" {
            (Some(&state.deletions), "server.deletions")
        } else if frame.method == "attachment.begin" {
            (Some(&state.upload_begins), "server.upload_begins")
        } else if frame.method == "artifact.read" {
            (Some(&state.artifact_reads), "server.artifact_reads")
        } else {
            (Some(&state.requests), "server.requests")
        };
        let permit = capacity.map(|capacity| capacity.clone().try_acquire_owned());
        let Ok(permit) = permit.transpose() else {
            note_limit(limit_id);
            let response = failure(&frame.id, "temporarily_unavailable");
            let queued = QueuedResponse {
                message: WireResponse::ordinary(response),
                _slot: slot,
                _record_work: None,
                _mount: mount_guard,
            };
            let rejected = if control {
                rejected_lane(
                    "socket.control_lane",
                    control_send.try_send(ControlOutput::Response(Box::new(queued))),
                )
            } else if record {
                rejected_lane(
                    "socket.record_lane",
                    record_send.try_send(QueuedRecordResponse::new(queued)),
                )
            } else {
                rejected_lane("socket.ordinary_lane", ordinary_send.try_send(queued))
            };
            if rejected {
                break;
            }
            continue;
        };
        let request_state = state.clone();
        let request_session = session.clone();
        let request_window = std::sync::Arc::downgrade(&window);
        let task = tokio::spawn(async move {
            if matches!(class, ResponseClass::Record) {
                let (message, record_work) = dispatch_passive_read(
                    &request_state,
                    &request_session,
                    frame,
                    // The same socket admission survives both response delivery
                    // and physical work, even after a delivered read_timeout (R61).
                    RecordReadLease::new((
                        permit.expect("record reads have capacity"),
                        slot.clone(),
                    )),
                    read_deadline,
                )
                .await;
                (message, class, slot, record_work, mount_guard)
            } else {
                let _permit = permit;
                // Weak: a request still running when the socket goes does not
                // keep its window published.
                let window = request_window.upgrade();
                let message = dispatch_on(
                    &request_state,
                    &request_session,
                    frame,
                    window.as_deref().and_then(Option::as_ref),
                )
                .await;
                (
                    WireResponse::ordinary(message),
                    class,
                    slot,
                    None,
                    mount_guard,
                )
            }
        });
        if app {
            app_calls.retain(|call| !call.is_finished());
            app_calls.push(task.abort_handle());
        }
        requests.push(task);
    }
    // Dropping the slot unsent lets the writer finish once the lanes drain.
    match close {
        Some(reason) => {
            let _ = close_send.send(reason);
        }
        None => drop(close_send),
    }
    for call in app_calls {
        call.abort();
    }
    drop(subscriptions);
    drop(control_send);
    drop(refusal_send);
    if let Some(task) = notice_task {
        task.abort();
        let _ = task.await;
    }
    drop(ordinary_send);
    drop(record_send);
    if !writer_finished {
        let _ = writer.await;
    }
}

/// Publish the reviews still waiting, then wait for the next change.
///
/// Each frame is the whole set, so a missed one is repaired by the next and
/// a surface that connects while a review is up sees it. The writer takes
/// the frame's sequence number as it sends, with the socket's other events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReviewNoticePublicationFailure {
    Audit(ReviewNoticeAuditFailure),
    Authority(AccessError),
    Unstable,
}

impl ReviewNoticePublicationFailure {
    fn close_reason(self) -> SessionCloseReason {
        match self {
            Self::Authority(error) => close_reason(error),
            Self::Audit(_) | Self::Unstable => SessionCloseReason::TemporaryUnavailable,
        }
    }
}

async fn forward_review_notices(
    board: Arc<crate::conversation::infrastructure::PermissionMemory>,
    send: mpsc::Sender<QueuedResponse>,
    slots: Arc<Semaphore>,
    state: ProductRouteState,
    session: AuthenticatedSession,
    mut authority_updates: watch::Receiver<u64>,
) -> Result<(), ReviewNoticePublicationFailure> {
    // Passes spent because access itself moved while selecting: who may see
    // what, or the authority revision. Those are rare, and each pass writes
    // audit records, so continuous change closes the socket. Reviews coming
    // and going on the board are ordinary traffic: under many busy agents the
    // board can change during every audit, and closing every socket for it
    // dropped clients mid-command. A pass the board alone outdated is
    // published, less what settled meanwhile.
    const MAX_SELECTION_PASSES: usize = 3;
    let mut selection_passes = 0;
    'snapshots: loop {
        if selection_passes == MAX_SELECTION_PASSES {
            return Err(ReviewNoticePublicationFailure::Unstable);
        }
        let (generation, reviews) = board.current();
        let visibility = board.visibility();
        // Observe before either await: a later authority update stays pending
        // while this admitted snapshot filters or waits for publication capacity.
        drop(authority_updates.borrow_and_update());
        let mut visible = Vec::with_capacity(reviews.len());
        let mut selected_revision = None;
        for review in reviews {
            let selection = super::read_access::may_see_review(&state, &session, &review)
                .await
                .map_err(ReviewNoticePublicationFailure::Audit)?;
            match selection {
                ReviewNoticeVisibility::Hidden => {}
                ReviewNoticeVisibility::Paired => visible.push(review),
                ReviewNoticeVisibility::Audited(selection) => {
                    let revision = selection.authority_revision();
                    if selected_revision.is_some_and(|earlier| earlier != revision) {
                        selection_passes += 1;
                        continue 'snapshots;
                    }
                    selected_revision = Some(revision);
                    if matches!(selection.decision(), AccessDecision::Allow { .. }) {
                        visible.push(review);
                    }
                }
            }
        }
        if let Some(revision) = selected_revision {
            let current = (ReadCurrentSession {
                access: state.access.as_ref(),
                clock: state.clock.as_ref(),
            })
            .execute(&session)
            .await
            .map_err(ReviewNoticePublicationFailure::Authority)?;
            if !board.is_current(generation, visibility) {
                // A spent counter never matches, so it counts as access moving.
                let board_moved = current.revision == revision
                    && board.visibility() == visibility
                    && generation != u64::MAX
                    && visibility != u64::MAX;
                if !board_moved {
                    selection_passes += 1;
                    continue 'snapshots;
                }
                // Only the board moved, with the same access: this selection
                // still holds for every review it kept that is still waiting,
                // so it is published without those that settled, under the
                // generation it was taken at. The board is past that
                // generation, so the next pass starts at once and adds what
                // came since. Redoing it instead never finished while busy
                // agents changed the board during every audit, and no card
                // was shown until its review was denied for want of an answer.
                let (_, waiting) = board.current();
                visible.retain(|review| waiting.contains(review));
            }
            if current.revision != revision {
                selection_passes += 1;
                continue 'snapshots;
            }
        }
        // Stable audited selection admits this candidate. As with other gateway
        // responses, its subsequent UI-capacity wait may finish after revocation;
        // an authority update stays pending for the next replacement.
        // One title read per conversation, however many of its reviews wait.
        let mut titles = HashMap::<String, Option<String>>::new();
        let mut reviews = Vec::with_capacity(visible.len());
        for review in visible {
            let title = match titles.get(&review.conversation_id) {
                Some(title) => title.clone(),
                None => {
                    let title = review_title(&state, &review.conversation_id).await;
                    titles.insert(review.conversation_id.clone(), title.clone());
                    title
                }
            };
            reviews.push(PermissionReviewNotice {
                conversation_id: review.conversation_id,
                execution_id: review.execution_id,
                permission_id: review.permission_id,
                tool: review.tool,
                title,
            });
        }
        let payload = fitted_review_set(reviews);
        let Ok(permit) = slots.clone().acquire_owned().await else {
            break;
        };
        let queued = QueuedResponse {
            message: WireResponse::ReviewWaiting {
                payload,
                generation,
            },
            _slot: Arc::new(permit),
            _record_work: None,
            _mount: None,
        };
        if send.send(queued).await.is_err() {
            break;
        }
        selection_passes = 0;
        tokio::select! {
            () = board.wait_after(generation) => {}
            () = board.wait_for_visibility(visibility) => {}
            changed = authority_updates.changed() => {
                if changed.is_err() {
                    break;
                }
            }
        }
    }
    Ok(())
}

/// The conversation's name as `conversation.list` shows it, so a notice can
/// name a conversation its client has not listed yet: a review can reach a
/// client before the list frame that names its conversation.
async fn review_title(state: &ProductRouteState, conversation_id: &str) -> Option<String> {
    let conversations = state.conversations.as_ref()?;
    let id = super::conversation::conversation_id(conversation_id).ok()?;
    conversations.title(&id).await
}

/// Room an event frame needs around its payload: its kind, name, number
/// and generation.
const REVIEW_FRAME_ROOM: usize = 512;

/// The waiting set, without titles when they would take it past one frame:
/// a frame over the limit closes the socket, and the client that reconnects
/// is sent the same set again. A notice without a title names the
/// conversation by id.
fn fitted_review_set(reviews: Vec<PermissionReviewNotice>) -> PermissionReviewWaiting {
    let mut set = PermissionReviewWaiting { reviews };
    let fits = serde_json::to_vec(&set)
        .is_ok_and(|bytes| bytes.len() + REVIEW_FRAME_ROOM <= MAX_PAYLOAD_BYTES as usize);
    if !fits {
        for review in &mut set.reviews {
            review.title = None;
        }
    }
    set
}

/// Give a waiting-review snapshot the next event number. Any other message
/// is unchanged. `None` when the numbering is spent: the socket keeps going,
/// and a later change of the set is sent with a number if one remains.
fn number_review_notice(message: WireResponse, sequence: &EventSequence) -> Option<WireResponse> {
    let WireResponse::ReviewWaiting {
        payload,
        generation,
    } = message
    else {
        return Some(message);
    };
    let seq = sequence.next()?;
    let frame = EventFrame::push(
        product_event::PERMISSION_REVIEW_WAITING,
        &payload,
        seq,
        generation,
    )
    .ok()?;
    Some(WireResponse::ordinary(OutgoingMessage::Event(frame)))
}

pub(super) fn valid_product_request(frame: &RequestFrame) -> bool {
    frame.kind == "req" && !frame.id.is_empty() && frame.id.len() <= 256
}

async fn dispatch_passive_read(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
    lease: RecordReadLease,
    deadline: Instant,
) -> (WireResponse, Option<RecordReadLease>) {
    let method = frame.method.clone();
    let answered = answer_passive_read(state, session, frame, lease, deadline).await;
    if let WireResponse::Ordinary(response) = &answered.0 {
        super::conversation::trace_refused_command(&method, response);
    }
    answered
}

async fn answer_passive_read(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
    lease: RecordReadLease,
    deadline: Instant,
) -> (WireResponse, Option<RecordReadLease>) {
    let request_id = frame.id.clone();
    let reading = async move {
        let (current, _) = match current_identity_inner(state, session).await {
            Ok(current) => current,
            Err(error) => return (passive_access_failure(&frame.id, error), None),
        };
        if !valid_product_request(&frame) {
            return (
                WireResponse::ordinary(failure(&frame.id, "invalid_request")),
                None,
            );
        }
        if let Err(error) = ensure_browser_session_present(state, &current).await {
            return (passive_access_failure(&frame.id, error), None);
        }
        if let Err(code) = super::owner_phone::admit_method(state, &current, &frame.method).await {
            return (WireResponse::ordinary(failure(&frame.id, code)), None);
        }
        let request_id = frame.id.clone();
        let result = if matches!(
            frame.method.as_str(),
            "conversation.catalogueHead"
                | "conversation.catalogueManifest"
                | "conversation.catalogueResolve"
        ) {
            super::catalogue_read::dispatch(state, &current, frame, lease)
                .await
                .map_err(|code| code.as_str())
        } else {
            super::record_read::dispatch(state, &current, frame, lease)
                .await
                .map_err(|code| code.as_str())
        };
        match result {
            Ok((text, lease)) => (WireResponse::record(text), Some(lease)),
            Err(code) => (WireResponse::ordinary(failure(&request_id, code)), None),
        }
    };
    let result = within_deadline(deadline, reading).await;
    result.unwrap_or_else(|| {
        (
            WireResponse::ordinary(failure(&request_id, "read_timeout")),
            None,
        )
    })
}

fn passive_access_failure(request_id: &str, error: AccessError) -> WireResponse {
    let code = refusal_code(access_refusal(error));
    WireResponse::ordinary(failure(request_id, code.as_str()))
}

/// A request on a connection with no window to publish: the tests' way in.
#[cfg(test)]
async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    dispatch_on(state, session, frame, None).await
}

/// [`dispatch`] on a connection that holds `window`, its place to publish
/// its window in.
async fn dispatch_on(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
    window: Option<&WindowSlot>,
) -> OutgoingMessage {
    let method = frame.method.clone();
    let response = answer_request(state, session, frame, window).await;
    super::conversation::trace_refused_command(&method, &response);
    response
}

async fn answer_request(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
    window: Option<&WindowSlot>,
) -> OutgoingMessage {
    let (session, snapshot) = match current_identity(state, session).await {
        Ok(current) => current,
        Err(_) => return failure(&frame.id, "unauthorized"),
    };
    if !valid_product_request(&frame) {
        return failure(&frame.id, "invalid_request");
    }
    if frame.method == PRODUCT_HANDSHAKE_METHOD {
        return failure(&frame.id, "already_authenticated");
    }
    if frame.method == "auth.session" {
        if let Err(code) = super::owner_phone::admit_method(state, &session, &frame.method).await {
            return failure(&frame.id, code);
        }
        if frame.params != json!({}) {
            return failure(&frame.id, "invalid_request");
        }
        if ensure_browser_session_present(state, &session)
            .await
            .is_err()
        {
            return failure(&frame.id, "unauthorized");
        }
        return success(&frame.id, &session_ready(state, &session, &snapshot));
    }
    match super::read_access::admit_request(state, &session, &frame).await {
        Ok(_) if frame.method == super::workspace::METHOD => {
            super::workspace::publish(window, frame)
        }
        Ok(initiation) => dispatch_authorized(state, &session, frame, initiation).await,
        Err(code) => failure(&frame.id, code),
    }
}

/// The one admission of an authenticated action, for a session whose
/// identity was just found current: the method's grant under current policy,
/// then browser presence. A request asks it once; a subscription asks it
/// before every batch it reads (`subscription::authorize_batch`).
pub(super) async fn admit_action(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    method: &str,
) -> Result<(), &'static str> {
    super::owner_phone::admit_method(state, session, method).await?;
    let action_name = action_for_method(method).ok_or("unknown_method")?;
    admit_grant(state, session, action_name).await
}

/// [`admit_action`] for a grant named directly: `action_name` under current
/// policy, then browser presence. A paired session's Drive command asks it
/// for pairing's own grant (`read_access::admit_request`).
pub(super) async fn admit_grant(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    action_name: &str,
) -> Result<(), &'static str> {
    match authorize(state, session, action_name).await {
        Ok(Decision::Allow) => ensure_browser_session_present(state, session)
            .await
            .map_err(|_| "unauthorized"),
        Ok(Decision::Deny) => Err("forbidden"),
        Err(_) => Err("unauthorized"),
    }
}

/// A request's admission outside `dispatch`: the session as it stands now,
/// then [`admit_action`] for `method`. The session found current.
///
/// Paired reads use [`read_access::admit_paired_read`]: pairing mints
/// `conversation.read`, while their manifest asks `conversation.write`.
/// Their target still asks [`read_access::admit_conversation`].
pub(super) async fn admit_now(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    method: &str,
) -> Result<AuthenticatedSession, &'static str> {
    let current = current_session_now(state, session).await?;
    if !super::read_access::admit_paired_read(state, &current, method).await? {
        admit_action(state, &current, method).await?;
    }
    Ok(current)
}

/// The session as it stands now, as a request finds it before it is
/// dispatched: refused `unauthorized` when it is no longer current.
pub(super) async fn current_session_now(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
) -> Result<AuthenticatedSession, &'static str> {
    current_identity(state, session)
        .await
        .map(|(current, _)| current)
        .map_err(|_| "unauthorized")
}

async fn authorize(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    action_name: &str,
) -> Result<Decision, AccessError> {
    AuthorizeAction {
        access: state.access.as_ref(),
        clock: state.clock.as_ref(),
        policy: state.policy.as_ref(),
    }
    .execute(
        session,
        &Action::new(action_name).expect("static action is valid"),
        &state.gateway,
    )
    .await
}

async fn set_native_hooks(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    use crate::agents::application::{
        NativeHooksAgent as Choice, NativeHooksChange, NativeHooksError,
    };
    use nessa_protocol::product::generated::{
        AgentsSetNativeHooksParams, AgentsSetNativeHooksResult,
    };
    let Some(hooks) = state.native_hooks.as_ref() else {
        return failure(&frame.id, "conversations_not_configured");
    };
    let params: AgentsSetNativeHooksParams = match serde_json::from_value(frame.params) {
        Ok(params) => params,
        Err(_) => return failure(&frame.id, "invalid_request"),
    };
    if params.request_id.chars().count() < MIN_NATIVE_HOOKS_REQUEST_ID_CHARACTERS
        || params.request_id.len() > MAX_NATIVE_HOOKS_REQUEST_ID_BYTES
    {
        return failure(&frame.id, "invalid_request");
    }
    let agent = Choice::from(params.agent);
    let context = session.context();
    let enabled = params.enabled;
    let request_id = params.request_id.clone();
    let caller_kind = state
        .caller_kinds
        .as_ref()
        .map(|source| {
            source
                .caller_kind(context.principal_id().as_str())
                .to_owned()
        })
        .unwrap_or_else(|| "unknown".to_owned());
    let change = NativeHooksChange {
        agent,
        enabled,
        request_id: params.request_id,
        organization_id: context.organization_id().as_str().to_owned(),
        principal_id: context.principal_id().as_str().to_owned(),
        surface_id: context.credential_id().as_str().to_owned(),
        caller_kind,
    };
    match hooks.set(change).await {
        Ok(selection) => success(
            &frame.id,
            &AgentsSetNativeHooksResult {
                request_id,
                agent: params.agent,
                enabled: selection.get(agent),
            },
        ),
        Err(NativeHooksError::Audit) => failure(&frame.id, "audit_unavailable"),
        // A crashed save is not a new wire code. The command can be retried.
        Err(NativeHooksError::Storage | NativeHooksError::Interrupted) => {
            failure(&frame.id, "temporarily_unavailable")
        }
    }
}

impl From<nessa_protocol::product::generated::NativeHooksAgent>
    for crate::agents::application::NativeHooksAgent
{
    fn from(agent: nessa_protocol::product::generated::NativeHooksAgent) -> Self {
        use nessa_protocol::product::generated::NativeHooksAgent as WireAgent;
        match agent {
            WireAgent::Claude => Self::Claude,
            WireAgent::Codex => Self::Codex,
        }
    }
}

async fn dispatch_authorized(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
    initiation: Initiation,
) -> OutgoingMessage {
    match frame.method.as_str() {
        "agents.list" => {
            if frame.params != json!({}) {
                return failure(&frame.id, "invalid_request");
            }
            match state.agents_catalog_now() {
                Some(catalog) => success(&frame.id, &catalog),
                None => failure(&frame.id, "conversations_not_configured"),
            }
        }
        "agents.setNativeHooks" => set_native_hooks(state, session, frame).await,
        "agents.installOptions" | "agents.install" => {
            super::agent_install::dispatch(state, session, frame).await
        }
        "conversation.share" | "conversation.unshare" | "conversation.shares" => {
            super::read_access::dispatch_share(state, session, frame).await
        }
        method if method.starts_with("conversation.") => {
            super::conversation::dispatch(state, session, frame, initiation).await
        }
        method if method.starts_with("attachment.") => {
            super::attachment::dispatch(state, session, frame).await
        }
        "artifact.read" => super::attachment::dispatch_read(state, session, frame).await,
        method if method.starts_with("mcp.") => {
            super::mcp_apps::dispatch(state, session, frame).await
        }
        method if method.starts_with("mcpServers.") => {
            super::mcp_servers::dispatch(state, session, frame).await
        }
        method if method.starts_with("github.") => {
            super::github::dispatch(state, session, frame).await
        }
        "pairing.setLinking" => super::pairing::set_linking(state, session, frame).await,
        method if method.starts_with("pairing.") => {
            super::pairing::dispatch(state, session, frame).await
        }
        "phone.endpoint" => super::phone_endpoint::dispatch(state, session, frame),
        "phone.authorize" | "phone.status" => super::phone::dispatch(state, session, frame).await,
        method if method.starts_with("peer.") => {
            super::peers::dispatch(state, session, frame).await
        }
        "server.health" => {
            if frame.params != json!({}) {
                return failure(&frame.id, "invalid_request");
            }
            health_check_message(&frame.id, state.uptime_clock.elapsed_ms())
                .unwrap_or_else(|_| failure(&frame.id, "internal_error"))
        }
        "credential.issue" => {
            let Some(admin) = state.admin.as_ref() else {
                return failure(&frame.id, "credential_store_unavailable");
            };
            let params: CredentialIssueParams = match serde_json::from_value(frame.params) {
                Ok(params) => params,
                Err(_) => return failure(&frame.id, "invalid_request"),
            };
            let context = session.context();
            let now = state.clock.unix_seconds();
            // Only the frame's shape is checked here. Who may be issued a
            // credential and which grants it may carry are the store's
            // questions, answered by Cedar's `credential.issue` and
            // `credential.hold` (gate 18): they are not repeated here.
            if params.request_id.is_empty()
                || params.request_id.len() > 200
                || params.membership.organization_id != context.organization_id().as_str()
                || params.membership.principal_id != params.principal.id
                || !matches!(params.membership.state, MembershipStateDto::Active)
                || params
                    .expires_at
                    .is_some_and(|expiry| expiry <= now || expiry > 9_007_199_254_740_991)
                || params.grants.is_empty()
            {
                return failure(&frame.id, "invalid_request");
            }
            let credential_id = format!("credential-{}", Uuid::new_v4());
            let outcome = admin
                .issue(IssueCredentialRequest {
                    request_id: params.request_id,
                    issuer_principal_id: context.principal_id().as_str().to_owned(),
                    credential_id,
                    principal: params.principal,
                    membership: params.membership,
                    audience_id: state.audience.as_str().to_owned(),
                    issued_at: now,
                    expires_at: params.expires_at,
                    grants: params.grants,
                })
                .await;
            match outcome {
                // Lifecycle evidence is committed durably with the registry state
                // itself; the wire result reports the credential, not the journal.
                Ok(IssueCredentialOutcome::Issued {
                    metadata, evidence, ..
                }) => {
                    let Ok(secret) = String::from_utf8(evidence.expose_bytes().to_vec()) else {
                        return failure(&frame.id, "internal_error");
                    };
                    success(
                        &frame.id,
                        &IssuedCredentialResult {
                            credential: metadata,
                            secret,
                        },
                    )
                }
                Ok(IssueCredentialOutcome::ExistingSecretUnavailable { metadata, .. }) => success(
                    &frame.id,
                    &ExistingCredentialResult {
                        credential: metadata,
                        secret_unavailable: true,
                    },
                ),
                Err(error) => failure(&frame.id, credential_admin_code(error)),
            }
        }
        "credential.list" => {
            let Some(admin) = state.admin.as_ref() else {
                return failure(&frame.id, "credential_store_unavailable");
            };
            if serde_json::from_value::<CredentialListParams>(frame.params).is_err() {
                return failure(&frame.id, "invalid_request");
            }
            match admin
                .list(ListCredentialsRequest {
                    organization_id: session.context().organization_id().as_str().to_owned(),
                })
                .await
            {
                Ok(credentials) => success(&frame.id, &CredentialListResult { credentials }),
                Err(error) => failure(&frame.id, credential_admin_code(error)),
            }
        }
        "credential.revoke" => {
            let Some(admin) = state.admin.as_ref() else {
                return failure(&frame.id, "credential_store_unavailable");
            };
            let params: CredentialRevokeParams = match serde_json::from_value(frame.params) {
                Ok(params) => params,
                Err(_) => return failure(&frame.id, "invalid_request"),
            };
            if params.request_id.trim().is_empty()
                || params.request_id.len() > 200
                || params.credential_id.is_empty()
            {
                return failure(&frame.id, "invalid_request");
            }
            let target_id = match CredentialId::new(&params.credential_id) {
                Ok(id) => id,
                Err(_) => return failure(&frame.id, "invalid_request"),
            };
            let target = match state.access.read(&target_id).await {
                Ok(snapshot) => snapshot,
                Err(AccessError::InvalidCredential) => {
                    return failure(&frame.id, "credential_not_found")
                }
                Err(_) => return failure(&frame.id, "credential_store_unavailable"),
            };
            if target.credential.organization_id() != session.context().organization_id()
                || target.credential.audience_id() != state.audience()
            {
                return failure(&frame.id, "forbidden");
            }
            match admin
                .revoke(RevokeCredentialRequest {
                    request_id: params.request_id,
                    issuer_principal_id: session.context().principal_id().as_str().to_owned(),
                    credential_id: params.credential_id.clone(),
                    revoked_at: state.clock.unix_seconds(),
                })
                .await
            {
                Ok(RevokeCredentialOutcome { revision, .. }) => success(
                    &frame.id,
                    &CredentialRevokeResult {
                        credential_id: params.credential_id,
                        revision,
                    },
                ),
                Err(error) => failure(&frame.id, credential_admin_code(error)),
            }
        }
        _ => failure(&frame.id, "unknown_method"),
    }
}

pub(super) fn success<T: serde::Serialize>(request_id: &str, payload: &T) -> OutgoingMessage {
    ResponseFrame::success(request_id, payload)
        .map(OutgoingMessage::Response)
        .unwrap_or_else(|_| failure(request_id, "internal_error"))
}

// Which request to blame for a frame that did not decode. Answer
// `invalid_request` when the envelope parser reads one JSON object, no decoded
// envelope name appears twice, `type` is `req`, and `id` is one Unicode string
// of 1 to 256 bytes. An envelope name that is not Unicode is skipped, so two
// of them are not a duplicate. A repeated name inside a nested value, or a
// nested string that is not Unicode, still leaves that id. Deeper than 127
// containers, the envelope is not read. Anything else gets no reply, and the
// caller's own timeout settles it.
fn correlatable_invalid_request(text: &str) -> Option<OutgoingMessage> {
    let value = unique_envelope(text).ok()?;
    let object = value.as_object()?;
    if object.get("type")?.as_str()? != "req" {
        return None;
    }
    let request_id = object.get("id")?.as_str()?;
    if request_id.is_empty() || request_id.len() > 256 {
        return None;
    }
    Some(failure(request_id, "invalid_request"))
}

fn session_ready(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    snapshot: &AccessSnapshot,
) -> ProductSessionReady {
    let grants = snapshot
        .credential
        .grants()
        .iter()
        .map(|grant| CredentialGrantDto {
            action: grant.action().as_str().to_owned(),
            resource: ResourceDto {
                organization_id: grant.resource().organization_id().as_str().to_owned(),
                id: grant.resource().id().as_str().to_owned(),
            },
        })
        .collect();
    ready_frame(
        state.gateway_id().as_str(),
        session,
        grants,
        PRODUCT_READY_METHODS
            .iter()
            .map(|method| (*method).to_owned())
            .collect(),
    )
}

#[cfg(test)]
async fn current_snapshot(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
) -> Result<AccessSnapshot, AccessError> {
    current_identity(state, session)
        .await
        .map(|(_, snapshot)| snapshot)
}

async fn current_identity(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
) -> Result<(AuthenticatedSession, AccessSnapshot), AccessError> {
    timeout(
        state.settings.handshake_timeout(),
        current_identity_inner(state, session),
    )
    .await
    .map_err(|_| AccessError::Unavailable)?
}

async fn current_identity_inner(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
) -> Result<(AuthenticatedSession, AccessSnapshot), AccessError> {
    if let Some(id) = &state.browser_session_id {
        let store = state
            .browser_sessions
            .as_ref()
            .ok_or(AccessError::InvalidCredential)?;
        let expected_origin = state
            .browser_session_origin
            .as_deref()
            .ok_or(AccessError::InvalidCredential)?;
        let retained = ReadBrowserSession {
            store: store.as_ref(),
        }
        .execute(id, state.clock.unix_seconds())
        .await?
        .filter(|session| session.origin() == expected_origin)
        .ok_or(AccessError::InvalidCredential)?;
        if retained.credential_id() != session.context().credential_id() {
            store
                .remove(
                    id.clone(),
                    state.clock.unix_seconds(),
                    RemovalReason::IdentityMismatch,
                    None,
                )
                .await?;
            return Err(AccessError::IdentityMismatch);
        }
        let evidence = SessionEvidence::new(id.as_bytes().to_vec())?;
        let verifier = BrowserSessionVerifier {
            store: store.as_ref(),
            expected_origin,
            now: state.clock.unix_seconds(),
        };
        match (ResumeSession {
            verifier: &verifier,
            access: state.access.as_ref(),
            clock: state.clock.as_ref(),
        })
        .execute(&evidence, &state.audience)
        .await
        {
            Ok(current) => return Ok(current),
            Err(error) => {
                if let Some(reason) = invalidation_reason(error) {
                    store
                        .remove(id.clone(), state.clock.unix_seconds(), reason, None)
                        .await?;
                }
                return Err(error);
            }
        }
    }
    let snapshot = ReadCurrentSession {
        access: state.access.as_ref(),
        clock: state.clock.as_ref(),
    }
    .execute(session)
    .await?;
    Ok((session.clone(), snapshot))
}

async fn ensure_browser_session_present(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
) -> Result<(), AccessError> {
    let Some(id) = &state.browser_session_id else {
        return Ok(());
    };
    let store = state
        .browser_sessions
        .as_ref()
        .ok_or(AccessError::InvalidCredential)?;
    let retained = ReadBrowserSession {
        store: store.as_ref(),
    }
    .execute(id, state.clock.unix_seconds())
    .await?
    .ok_or(AccessError::InvalidCredential)?;
    if retained.credential_id() != session.context().credential_id() {
        store
            .remove(
                id.clone(),
                state.clock.unix_seconds(),
                RemovalReason::IdentityMismatch,
                None,
            )
            .await?;
        return Err(AccessError::IdentityMismatch);
    }
    Ok(())
}

async fn current_session_error(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
) -> Option<AccessError> {
    current_identity(state, session).await.err()
}

pub(super) fn failure(request_id: &str, code: &str) -> OutgoingMessage {
    OutgoingMessage::Response(ResponseFrame::failure(request_id, code, code))
}

pub(super) fn failure_with_details(
    request_id: &str,
    code: &str,
    details: serde_json::Value,
) -> OutgoingMessage {
    OutgoingMessage::Response(ResponseFrame::failure_with_details(
        request_id,
        code,
        code,
        Some(details),
    ))
}

async fn send_error<S: Sink<Message> + Unpin>(
    write_timeout: Duration,
    socket: &mut S,
    request_id: &str,
    code: &str,
) -> Result<(), ()> {
    send(write_timeout, socket, failure(request_id, code)).await
}

async fn send<S: Sink<Message> + Unpin>(
    write_timeout: Duration,
    socket: &mut S,
    message: OutgoingMessage,
) -> Result<(), ()> {
    let text = ordinary_text(message)?;
    write_budget(timeout(write_timeout, send_text(socket, text)).await)?
}

/// An elapsed write is `socket.write_timeout`. A finished send keeps its result.
/// `a_stalled_write_names_the_write_timeout`, `a_stalled_close_names_the_write_timeout`.
fn write_budget<T>(result: Result<T, Elapsed>) -> Result<T, ()> {
    result.map_err(|_| {
        note_limit("socket.write_timeout");
    })
}

/// An ordinary message's wire text, refused past the ordinary encoded ceiling.
fn ordinary_text(message: OutgoingMessage) -> Result<String, ()> {
    let text = message.to_wire_text().map_err(|_| ())?;
    if text.len() > MAX_PAYLOAD_BYTES as usize {
        note_limit("product.max_payload_bytes");
        return Err(());
    }
    Ok(text)
}

async fn send_queued<S: Sink<Message> + Unpin>(
    write_timeout: Duration,
    socket: &mut S,
    message: WireResponse,
) -> Result<(), ()> {
    match message {
        WireResponse::Ordinary(message) => send(write_timeout, socket, *message).await,
        WireResponse::Record { text } => {
            if text.len() > MAX_RECORD_RESPONSE_BYTES {
                note_limit("product.max_record_response_bytes");
                return Err(());
            }
            socket
                .send(Message::Text(text.into()))
                .await
                .map_err(|_| ())
        }
        // Numbered before it reaches here. A snapshot that was not is not a
        // record and is not sent on this path.
        WireResponse::ReviewWaiting { .. } => Err(()),
    }
}

async fn send_record_queued<S: Sink<Message> + Unpin>(
    write_timeout: Duration,
    socket: &mut S,
    response: QueuedRecordResponse,
) -> Result<(), ()> {
    let QueuedRecordResponse {
        message,
        _slot: slot,
        _record_work: record_work,
        _mount: mount,
        deadline,
    } = response;
    // The response's slot and read lease are given back as soon as its frame
    // is encoded and checked, before any call that can write a byte: a
    // WebSocket can write inside `start_send` (a pong due after the peer's
    // ping), so no flush order is relied on. After physical source completion,
    // a client waiting for the answer finds capacity free for its next read
    // (row R64). An encoding refusal gives delivery ownership back at once.
    let text = match message {
        WireResponse::Ordinary(message) => {
            let text = ordinary_text(*message)?;
            drop((slot, record_work, mount));
            return match within_deadline(deadline, timeout(write_timeout, send_text(socket, text)))
                .await
            {
                None => {
                    note_limit("socket.record_delivery_deadline");
                    Err(())
                }
                Some(timed) => write_budget(timed)?.map_err(|_| ()),
            };
        }
        WireResponse::Record { text } => text,
        // The record lane carries the five passive methods' answers only:
        // review snapshots go on the ordinary lane, so this arm is never
        // reached. It is still delivered as `send_queued` would, under this
        // response's deadline, rather than dropped.
        message @ WireResponse::ReviewWaiting { .. } => {
            drop((slot, record_work, mount));
            return missed_record_deadline(
                within_deadline(deadline, send_queued(write_timeout, socket, message)).await,
            )
            .ok_or(())?;
        }
    };
    if text.len() > MAX_RECORD_RESPONSE_BYTES {
        note_limit("product.max_record_response_bytes");
        return Err(());
    }
    drop((slot, record_work, mount));
    missed_record_deadline(within_deadline(deadline, send_text(socket, text)).await).ok_or(())?
}

/// Send one text frame and flush it.
async fn send_text<S: Sink<Message> + Unpin>(socket: &mut S, text: String) -> Result<(), ()> {
    socket
        .send(Message::Text(text.into()))
        .await
        .map_err(|_| ())
}

/// The one mapping from an access error to how a live connection closes.
pub(super) fn close_reason(error: AccessError) -> SessionCloseReason {
    match error {
        AccessError::CredentialRevoked => SessionCloseReason::CredentialRevoked,
        AccessError::CredentialExpired => SessionCloseReason::CredentialExpired,
        AccessError::Unavailable | AccessError::StaleRevision => {
            SessionCloseReason::TemporaryUnavailable
        }
        _ => SessionCloseReason::AuthorizationLost,
    }
}

fn retryable_access_error(error: AccessError) -> bool {
    matches!(error, AccessError::Unavailable | AccessError::StaleRevision)
}

async fn close_session<S: Sink<Message> + Unpin>(
    write_timeout: Duration,
    socket: &mut S,
    reason: SessionCloseReason,
) {
    let close = Message::Close(Some(CloseFrame {
        code: reason.web_socket_code(),
        reason: serde_json::to_string(&SessionTermination {
            code: reason,
            retryable: reason.retryable(),
            retry_after_ms: None,
        })
        .expect("fixed termination serializes")
        .into(),
    }));
    let _ = write_budget(timeout(write_timeout, socket.send(close)).await);
}

#[cfg(test)]
pub(crate) use tests::host_fixture::HostProductFixture;

#[cfg(test)]
mod tests {
    use super::super::state::SessionSettings;
    use super::*;

    /// A writer's close slot whose owner ended without a reason.
    fn no_close() -> oneshot::Receiver<SessionCloseReason> {
        oneshot::channel().1
    }
    use crate::agents_test_support::StubAgentProbe;
    use crate::browser_session::application::SessionStore;
    use crate::browser_session::domain::value_objects::{
        BrowserSessionOrigin, BrowserSessionState,
    };
    use crate::conversation::application::{
        AccessAction, AccessAsk, AccessDecision, AccessDenial, AccessSubject, CatalogueReadFuture,
        CatalogueReadOperation, CatalogueReadSource, ConversationAccessPolicyEvaluator,
        ConversationError, ConversationFuture, ConversationRepository, HeldGrant,
        PermissionRuleCatalog, ReceiverAuthority, ReceiverBinding, RecordReadError,
        RecordReadFuture, RecordReadOperation, RecordReadResponse, RecordReadSource,
        ReviewNoticeAuditor, RevisionHold, RulePrincipal, ShareGrantChange, ShareGrantList,
        ShareGrantReceipt, ShareGrants,
    };
    use crate::conversation::domain::{Capabilities, Conversation, ToolPolicy};
    use crate::conversation::infrastructure::{
        LocalConversationStore, MonotonicReviewNoticeClock, NessaRecordReadSource,
    };
    use crate::product::ProductDependencies;
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use conversation_support::MemoryRepository;
    use nessa_auth::adapters::cedar::CedarPolicyEvaluator;
    use nessa_auth::application::credential_admin::CredentialAdmin;
    use nessa_auth::application::dto::CredentialMetadataDto;
    use nessa_auth::application::ports::{
        AccessReader, AccessSnapshot, Clock, CredentialVerifier, PolicyEvaluator, PortFuture,
        VerifiedCredential,
    };
    use nessa_auth::domain::{
        AudienceId, AuthContext, Credential, CredentialId, Grant, Membership, MembershipId,
        MembershipRole, MembershipStatus, OrganizationId, PrincipalId, Resource, ResourceId,
    };
    use nessa_protocol::agents::AgentId;
    use nessa_protocol::clock::Clock as UptimeClock;
    use nessa_protocol::conversation::domain::{
        ConversationApprovalMode, ConversationId, ConversationModelId,
    };
    use nessa_protocol::conversation::read_scope::{ReadRefusal, ReceiverReadScope};
    use nessa_protocol::product::generated::{
        ConversationRecordsPageResult, RecordPageRequest, RecordScope, RecordWireRecord,
        MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
    };
    use nessa_protocol::product::passive_read::encode_response;
    use nessa_sdk::application::agent_execution::permissions::{
        ActionContext, PermissionCaller, PermissionRules, RememberRequest, ReviewNotice,
        ReviewNotices,
    };
    use nessa_sdk::application::agent_execution::providers::ProviderIdentity;
    use nessa_sdk::application::agent_execution::sessions::{
        ProviderContext, SessionChange, SessionSaveUnit, SessionSnapshot, SessionStorage,
    };
    use nessa_sdk::domain::agent_execution::executions::ExecutionId;
    use nessa_sdk::domain::agent_execution::permissions::{PermissionEffect, PermissionId};
    use nessa_sdk::domain::agent_execution::sessions::SessionId;
    use nessa_sdk::infrastructure::session_storage::{
        RecordStorage, MAX_PHYSICAL_RECORD_PAYLOAD_BYTES as SDK_MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
    };
    use nessa_sync::replication::domain::Id;
    use serde_json::Value;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::io::DuplexStream;
    use tokio::runtime::Handle;
    use tokio::sync::oneshot;
    use tokio_tungstenite::tungstenite::{protocol::Role, Message as Frame};
    use tokio_tungstenite::WebSocketStream;
    use uuid::Uuid;

    /// No receiver binding: the session is an owner surface or a local member.
    struct UnpairedBinding;
    impl ReceiverAuthority for UnpairedBinding {
        fn resolve<'a>(
            &'a self,
            _: &'a CredentialId,
        ) -> Pin<Box<dyn Future<Output = Result<Option<ReceiverBinding>, ReadRefusal>> + Send + 'a>>
        {
            Box::pin(async { Ok(None) })
        }
    }

    /// A paired device whose binding owner is `owner`, signed in on this credential.
    struct DeviceBinding;
    impl ReceiverAuthority for DeviceBinding {
        fn resolve<'a>(
            &'a self,
            _: &'a CredentialId,
        ) -> Pin<Box<dyn Future<Output = Result<Option<ReceiverBinding>, ReadRefusal>> + Send + 'a>>
        {
            Box::pin(async {
                Ok(Some(ReceiverBinding {
                    receiver_id: "device".into(),
                    credential_id: CredentialId::new("credential").unwrap(),
                    organization_id: OrganizationId::new("organization").unwrap(),
                    owner_id: PrincipalId::new("owner").unwrap(),
                    access_epoch: 3,
                    active: true,
                }))
            })
        }
    }

    /// A read grant on one conversation, for receiver `device` only.
    struct GrantedConversation(String);
    impl ShareGrants for GrantedConversation {
        fn held_grant<'a>(
            &'a self,
            id: &'a ConversationId,
            receiver_id: &'a str,
        ) -> ConversationFuture<'a, Option<HeldGrant>> {
            let allowed = receiver_id == "device" && id.to_string() == self.0;
            Box::pin(async move {
                Ok(allowed.then_some(HeldGrant {
                    capabilities: Capabilities::read(),
                    policy_revision: 1,
                }))
            })
        }
        fn change(&self, _: ShareGrantChange) -> ConversationFuture<'_, ShareGrantReceipt> {
            Box::pin(async { Err(ConversationError::Metadata) })
        }
        fn grants<'a>(&'a self, _: &'a ConversationId) -> ConversationFuture<'a, ShareGrantList> {
            Box::pin(async { Err(ConversationError::Metadata) })
        }
        fn policy_at<'a>(
            &'a self,
            _: &'a ConversationId,
            _: &'a str,
            _: u64,
        ) -> ConversationFuture<'a, Option<ToolPolicy>> {
            Box::pin(async { Err(ConversationError::Metadata) })
        }
        fn hold_revision<'a>(
            &'a self,
            _: &'a ConversationId,
            _: &'a str,
            _: u64,
        ) -> ConversationFuture<'a, RevisionHold> {
            Box::pin(async { Err(ConversationError::Metadata) })
        }
    }

    /// A read grant that a test can take away while a review is still waiting.
    struct ToggleGrant {
        conversation: String,
        open: Arc<AtomicBool>,
    }
    impl ShareGrants for ToggleGrant {
        fn held_grant<'a>(
            &'a self,
            id: &'a ConversationId,
            receiver_id: &'a str,
        ) -> ConversationFuture<'a, Option<HeldGrant>> {
            let allowed = self.open.load(Ordering::SeqCst)
                && receiver_id == "device"
                && id.to_string() == self.conversation;
            Box::pin(async move {
                Ok(allowed.then_some(HeldGrant {
                    capabilities: Capabilities::read(),
                    policy_revision: 1,
                }))
            })
        }
        fn change(&self, _: ShareGrantChange) -> ConversationFuture<'_, ShareGrantReceipt> {
            Box::pin(async { Err(ConversationError::Metadata) })
        }
        fn grants<'a>(&'a self, _: &'a ConversationId) -> ConversationFuture<'a, ShareGrantList> {
            Box::pin(async { Err(ConversationError::Metadata) })
        }
        fn policy_at<'a>(
            &'a self,
            _: &'a ConversationId,
            _: &'a str,
            _: u64,
        ) -> ConversationFuture<'a, Option<ToolPolicy>> {
            Box::pin(async { Err(ConversationError::Metadata) })
        }
        fn hold_revision<'a>(
            &'a self,
            _: &'a ConversationId,
            _: &'a str,
            _: u64,
        ) -> ConversationFuture<'a, RevisionHold> {
            Box::pin(async { Err(ConversationError::Metadata) })
        }
    }

    struct RecordBinding;
    impl ReceiverAuthority for RecordBinding {
        fn resolve<'a>(
            &'a self,
            _: &'a CredentialId,
        ) -> Pin<Box<dyn Future<Output = Result<Option<ReceiverBinding>, ReadRefusal>> + Send + 'a>>
        {
            Box::pin(async {
                Ok(Some(ReceiverBinding {
                    receiver_id: "receiver".into(),
                    credential_id: CredentialId::new("credential").unwrap(),
                    organization_id: OrganizationId::new("organization").unwrap(),
                    owner_id: PrincipalId::new("principal").unwrap(),
                    access_epoch: 3,
                    active: true,
                }))
            })
        }
    }

    struct RecordAdmissionSpy(AtomicU64);
    impl RecordReadSource for RecordAdmissionSpy {
        fn read<'a>(
            &'a self,
            _: ReceiverReadScope,
            _: RecordReadOperation,
            _: RecordReadLease,
        ) -> RecordReadFuture<'a, RecordReadResponse> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(RecordReadError::TemporarilyUnavailable) })
        }
    }
    mod writer {
        include!("../../tests/product/socket/writer.rs");
    }
    mod review_notice_audit {
        include!("../../tests/product/socket/review_notice_audit.rs");
    }
    pub(super) mod host_fixture {
        include!("../../tests/product/socket/host_fixture.rs");
    }
    mod subscriptions {
        include!("../../tests/product/socket/subscriptions.rs");
    }
    mod browser_sessions {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/browser_session/tests.rs"
        ));
    }

    #[test]
    fn every_manifest_method_has_one_runtime_dispatch_path() {
        let manifest: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../protocol/product/manifest.json"
        )))
        .unwrap();
        let advertised = manifest["methods"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        let runtime = std::iter::once(PRODUCT_HANDSHAKE_METHOD)
            .chain(PRODUCT_READY_METHODS.iter().copied())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(advertised, runtime);
        for method in PRODUCT_READY_METHODS {
            if *method == "auth.session" {
                continue;
            }
            assert!(
                action_for_method(method).is_some(),
                "advertised method has no authorization/dispatch path: {method}"
            );
        }
    }

    /// The removed change-watch methods are not part of the product: on an
    /// authenticated socket each is refused `unknown_method` by the ordinary
    /// dispatch path, and the socket stays open for the next request.
    #[tokio::test]
    async fn a_removed_watch_method_is_refused_as_unknown() {
        let (state, _) = fixture(MembershipRole::Member);
        let session = authenticate(&state).await;
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, state, session));
        let request = |peer: &TestPeer, id: &str, method: &str| {
            peer.input
                .send(Ok(Message::Text(
                    json!({"type": "req", "id": id, "method": method, "params": {}})
                        .to_string()
                        .into(),
                )))
                .unwrap();
        };
        let response = async |peer: &mut TestPeer| -> Value {
            let Message::Text(text) = peer.message().await else {
                panic!("a response")
            };
            serde_json::from_str(&text).unwrap()
        };
        for method in [
            "conversation.watchRecords",
            "conversation.watchCatalogue",
            "conversation.unwatch",
        ] {
            request(&peer, method, method);
            let refused = response(&mut peer).await;
            assert_eq!(refused["id"], method, "{refused}");
            assert_eq!(refused["ok"], false, "{refused}");
            assert_eq!(refused["error"]["code"], "unknown_method", "{refused}");
        }
        request(&peer, "health", "server.health");
        let health = response(&mut peer).await;
        assert_eq!(health["id"], "health", "{health}");
        assert_eq!(health["ok"], true, "{health}");
        drop(peer);
        timeout(Duration::from_secs(1), task)
            .await
            .expect("the socket ends with its peer")
            .unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn host_cleanup_closes_every_authenticated_socket_retryably() {
        let (state, _) = fixture(MembershipRole::Member);
        let session = authenticate(&state).await;
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, state.clone(), session));
        state.close_sockets();
        let Message::Close(Some(close)) = peer.message().await else {
            panic!("close expected")
        };
        assert_eq!(
            close.code,
            SessionCloseReason::TemporaryUnavailable.web_socket_code()
        );
        timeout(Duration::from_secs(1), task)
            .await
            .expect("the socket ends once closed")
            .unwrap();
    }

    /// The manifest is the only declaration of each method's grant.
    /// A method with no `grant` field fails here, and so does a generated
    /// mapping that answers something else.
    #[test]
    fn action_for_method_is_the_manifest_grant() {
        let manifest: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../protocol/product/manifest.json"
        )))
        .unwrap();
        for (method, spec) in manifest["methods"].as_object().unwrap() {
            assert!(
                spec.get("grant").is_some(),
                "{method} has no declared grant"
            );
            assert_eq!(
                action_for_method(method),
                spec["grant"].as_str(),
                "{method}"
            );
        }
    }

    struct FixedClock;
    impl Clock for FixedClock {
        fn unix_milliseconds(&self) -> u64 {
            1
        }
    }

    #[tokio::test]
    async fn a_new_session_is_told_when_no_review_is_waiting() {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("rules");
        nessa_local_storage::create_directory(&private).unwrap();
        let clock = Arc::new(FixedClock);
        let board = Arc::new(
            crate::conversation::infrastructure::PermissionMemory::open(
                &private.join("permission-rules.sqlite3"),
                clock,
            )
            .unwrap(),
        );
        let (send, mut recv) = mpsc::channel(1);
        let slots = Arc::new(Semaphore::new(1));
        let (state, _) = fixture(MembershipRole::Admin);
        let session = authenticate(&state).await;
        let (_authority, authority_updates) = watch::channel(session.auth_revision());
        let task = tokio::spawn(super::forward_review_notices(
            board,
            send,
            slots,
            state,
            session,
            authority_updates,
        ));
        let queued = tokio::time::timeout(std::time::Duration::from_secs(2), recv.recv())
            .await
            .expect("the empty board is sent")
            .expect("the notice task stays up");
        match queued.message {
            WireResponse::ReviewWaiting {
                payload,
                generation,
            } => {
                assert!(payload.reviews.is_empty());
                assert_eq!(generation, 0);
            }
            _ => panic!("expected the empty board"),
        }
        drop(recv);
        task.abort();
        let _ = task.await;
    }

    fn open_rules() -> (
        tempfile::TempDir,
        Arc<crate::conversation::infrastructure::PermissionMemory>,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("rules");
        nessa_local_storage::create_directory(&private).unwrap();
        let board = crate::conversation::infrastructure::PermissionMemory::open(
            &private.join("permission-rules.sqlite3"),
            Arc::new(FixedClock),
        )
        .unwrap();
        (directory, Arc::new(board))
    }

    fn owned_conversation(id: &str, owner: &str) -> Conversation {
        Conversation::new(
            ConversationId::new(id).unwrap(),
            OrganizationId::new("organization").unwrap(),
            PrincipalId::new(owner).unwrap(),
            "panel".into(),
            "create".into(),
            1,
            AgentId::Claude,
            ConversationModelId::new("model").unwrap(),
            ConversationApprovalMode::Ask,
        )
        .unwrap()
    }

    fn repository_of(rows: Vec<Conversation>) -> Arc<MemoryRepository> {
        let repository = MemoryRepository::default();
        {
            let mut records = repository.records.lock().unwrap();
            for conversation in rows {
                records.insert(conversation.id().clone(), conversation);
            }
        }
        Arc::new(repository)
    }

    async fn notice_snapshot(
        board: Arc<crate::conversation::infrastructure::PermissionMemory>,
        state: ProductRouteState,
        session: AuthenticatedSession,
    ) -> nessa_protocol::product::generated::PermissionReviewWaiting {
        let (send, mut recv) = mpsc::channel(1);
        let slots = Arc::new(Semaphore::new(1));
        let (_authority, authority_updates) = watch::channel(session.auth_revision());
        let task = tokio::spawn(super::forward_review_notices(
            board,
            send,
            slots,
            state,
            session,
            authority_updates,
        ));
        let queued = tokio::time::timeout(std::time::Duration::from_secs(2), recv.recv())
            .await
            .expect("a board is sent")
            .expect("the notice task stays up");
        drop(recv);
        task.abort();
        let _ = task.await;
        match queued.message {
            WireResponse::ReviewWaiting { payload, .. } => payload,
            _ => panic!("expected the board"),
        }
    }

    async fn drive_review_publication(
        publisher: Pin<&mut impl Future<Output = Result<(), ReviewNoticePublicationFailure>>>,
        receive: &mut mpsc::Receiver<QueuedResponse>,
    ) -> QueuedResponse {
        timeout(Duration::from_secs(2), async {
            tokio::select! {
                queued = receive.recv() => queued.expect("the authority update is published"),
                result = publisher => panic!("publisher ended before its snapshot: {result:?}"),
            }
        })
        .await
        .expect("the admitted snapshot is published")
    }

    fn code_of(message: OutgoingMessage) -> String {
        let OutgoingMessage::Response(response) = message else {
            panic!("response expected");
        };
        assert!(!response.ok);
        response.error.unwrap().code
    }

    /// An unpaired member who can write conversations is still a grant.
    /// Admission's owner turn is not the rule principal.
    #[tokio::test]
    async fn an_unpaired_member_cannot_list_or_revoke_remembered_rules() {
        let (_directory, board) = open_rules();
        let execution = ExecutionId::new("turn").unwrap();
        board.arm("conversation", &execution, true);
        let stored = board
            .remember(RememberRequest {
                workspace: "/work",
                source: "claude",
                tool: "Bash",
                effect: PermissionEffect::Allow,
                granted_by: &ActionContext::new("owner", "surface", "request").unwrap(),
                caller: PermissionCaller::Owner,
                conversation_id: "conversation",
                execution_id: &execution,
            })
            .unwrap();
        let rule_id = stored.reference().rule_id().to_owned();
        let (state, authority) = fixture(MembershipRole::Member);
        *authority.snapshot.lock().unwrap() = snapshot_with(
            MembershipRole::Member,
            MembershipStatus::Active,
            &["conversation.write"],
        );
        let state = state.with_permission_rules(board.clone());
        let session = authenticate(&state).await;
        let mut list = request("list", "conversation.permissionRules");
        list.params = json!({ "requestId": "list" });
        assert_eq!(code_of(dispatch(&state, &session, list).await), "forbidden");
        let mut revoke = request("revoke", "conversation.revokePermissionRule");
        revoke.params = json!({ "requestId": "revoke", "ruleId": rule_id });
        assert_eq!(
            code_of(dispatch(&state, &session, revoke).await),
            "forbidden"
        );
        assert_eq!(board.list(RulePrincipal::Owner).unwrap().len(), 1);
        *authority.snapshot.lock().unwrap() = snapshot_with(
            MembershipRole::Admin,
            MembershipStatus::Active,
            &["conversation.write"],
        );
        let mut list = request("list-owner", "conversation.permissionRules");
        list.params = json!({ "requestId": "list-owner" });
        let OutgoingMessage::Response(response) = dispatch(&state, &session, list).await else {
            panic!("response expected");
        };
        assert!(response.ok);
        assert_eq!(
            response.payload.unwrap()["rules"].as_array().unwrap().len(),
            1
        );
    }

    /// The waiting-review board is global. Each socket is told only the
    /// reviews in conversations that principal may read.
    #[tokio::test]
    async fn a_review_notice_names_only_a_conversation_the_recipient_may_read() {
        let (_directory, board) = open_rules();
        let id_a = "00000000-0000-4000-8000-00000000000a";
        let id_b = "00000000-0000-4000-8000-00000000000b";
        let execution = ExecutionId::new("turn").unwrap();
        board.waiting(&ReviewNotice {
            conversation_id: id_a.into(),
            execution_id: execution.clone(),
            permission_id: PermissionId::new("perm-a").unwrap(),
            tool: "Bash".into(),
        });
        board.waiting(&ReviewNotice {
            conversation_id: id_b.into(),
            execution_id: execution,
            permission_id: PermissionId::new("perm-b").unwrap(),
            tool: "Write".into(),
        });
        let foreign = repository_of(vec![
            owned_conversation(id_a, "owner"),
            owned_conversation(id_b, "owner"),
        ]);
        let mine = repository_of(vec![
            owned_conversation(id_a, "principal"),
            owned_conversation(id_b, "owner"),
        ]);
        let policy = crate::conversation_test_support::access_policy();

        let member_sees = async |actions: &[&str],
                                 receivers: Arc<dyn ReceiverAuthority>,
                                 conversations: Arc<MemoryRepository>,
                                 grants: Arc<dyn ShareGrants>,
                                 role: MembershipRole| {
            let (state, authority) = fixture(role);
            *authority.snapshot.lock().unwrap() =
                snapshot_with(role, MembershipStatus::Active, actions);
            let state = state.with_passive_read(receivers, conversations, grants, policy.clone());
            let session = authenticate(&state).await;
            notice_snapshot(board.clone(), state, session).await
        };

        let read_only = member_sees(
            &["server.read"],
            Arc::new(UnpairedBinding),
            foreign.clone(),
            Arc::new(GrantedConversation(id_a.into())),
            MembershipRole::Member,
        )
        .await;
        assert!(
            read_only.reviews.is_empty(),
            "a server.read credential sees no other conversation's reviews"
        );

        let writer = member_sees(
            &["conversation.write"],
            Arc::new(UnpairedBinding),
            foreign.clone(),
            Arc::new(GrantedConversation(id_a.into())),
            MembershipRole::Member,
        )
        .await;
        assert!(
            writer.reviews.is_empty(),
            "conversation.write does not reveal someone else's reviews"
        );

        let owner_surface = member_sees(
            &["server.read", "credential.manage"],
            Arc::new(UnpairedBinding),
            foreign.clone(),
            Arc::new(GrantedConversation(id_a.into())),
            MembershipRole::Admin,
        )
        .await;
        let mut tools = owner_surface
            .reviews
            .iter()
            .map(|review| review.tool.as_str())
            .collect::<Vec<_>>();
        tools.sort_unstable();
        assert_eq!(tools, ["Bash", "Write"]);

        let one_grant = member_sees(
            &["conversation.read"],
            Arc::new(DeviceBinding),
            foreign.clone(),
            Arc::new(GrantedConversation(id_a.into())),
            MembershipRole::Member,
        )
        .await;
        assert_eq!(one_grant.reviews.len(), 1);
        assert_eq!(one_grant.reviews[0].conversation_id, id_a);
        assert_eq!(one_grant.reviews[0].tool, "Bash");

        let paired_admin = member_sees(
            &["conversation.read", "credential.manage"],
            Arc::new(DeviceBinding),
            foreign,
            Arc::new(GrantedConversation(id_a.into())),
            MembershipRole::Admin,
        )
        .await;
        assert_eq!(paired_admin.reviews.len(), 1);
        assert_eq!(paired_admin.reviews[0].conversation_id, id_a);
        assert_eq!(paired_admin.reviews[0].tool, "Bash");

        let owns_one = member_sees(
            &["conversation.write"],
            Arc::new(UnpairedBinding),
            mine,
            Arc::new(GrantedConversation(id_a.into())),
            MembershipRole::Member,
        )
        .await;
        assert_eq!(owns_one.reviews.len(), 1);
        assert_eq!(owns_one.reviews[0].conversation_id, id_a);
        assert_eq!(owns_one.reviews[0].tool, "Bash");
    }

    #[tokio::test(start_paused = true)]
    async fn a_live_review_publisher_refilters_after_authority_refresh() {
        for refresh_before_demotion in [false, true] {
            let (_directory, board) = open_rules();
            let own = "00000000-0000-4000-8000-00000000000a";
            let foreign = "00000000-0000-4000-8000-00000000000b";
            for id in [own, foreign] {
                board.waiting(&ReviewNotice {
                    conversation_id: id.into(),
                    execution_id: ExecutionId::new("turn").unwrap(),
                    permission_id: PermissionId::new(id).unwrap(),
                    tool: "Bash".into(),
                });
            }
            let (state, authority) = fixture(MembershipRole::Admin);
            let state = state
                .with_permission_rules(board.clone())
                .with_passive_read(
                    Arc::new(UnpairedBinding),
                    repository_of(vec![
                        owned_conversation(own, "principal"),
                        owned_conversation(foreign, "other"),
                    ]),
                    Arc::new(GrantedConversation(own.into())),
                    crate::conversation_test_support::access_policy(),
                )
                .with_settings(
                    SessionSettings::new(
                        Duration::from_secs(20),
                        Duration::from_secs(5),
                        Duration::from_secs(10),
                    )
                    .unwrap(),
                );
            let session = authenticate(&state).await;
            let (socket, mut peer) = test_socket(None);
            let task = tokio::spawn(run_authenticated(socket, state, session));
            let read_notice = async |peer: &mut TestPeer| {
                let message = timeout(Duration::from_secs(1), peer.output.recv())
                    .await
                    .expect("authority refresh must replace the live review snapshot")
                    .expect("the connection remains open");
                let Message::Text(text) = message else {
                    panic!("review event expected")
                };
                let OutgoingMessage::Event(event) = serde_json::from_str(&text).unwrap() else {
                    panic!("review event expected")
                };
                assert_eq!(event.event, product_event::PERMISSION_REVIEW_WAITING);
                let payload: PermissionReviewWaiting =
                    serde_json::from_value(event.payload).unwrap();
                (event.state_version, event.seq, payload.reviews)
            };
            let (generation, mut sequence, initial) = read_notice(&mut peer).await;
            assert_eq!(initial.len(), 2);
            if refresh_before_demotion {
                let visibility = board.visibility();
                tokio::time::advance(Duration::from_secs(10)).await;
                assert!(
                    timeout(Duration::from_secs(1), peer.output.recv())
                        .await
                        .is_err(),
                    "the authentication revision must not republish the snapshot"
                );
                assert_eq!(board.visibility(), visibility);
            }
            // The board and share state stay unchanged throughout. Keep the same
            // connection/publisher alive across first refresh, promotion and demotion.
            let replace_membership = |role, status| {
                let mut current = authority.snapshot.lock().unwrap();
                let mut next = snapshot(role, status);
                next.revision = current.revision + 1;
                *current = next;
            };
            for (role, expected) in [
                (MembershipRole::Member, vec![own]),
                (MembershipRole::Member, vec![own]),
                (MembershipRole::Admin, vec![own, foreign]),
                (MembershipRole::Member, vec![own]),
            ] {
                replace_membership(role, MembershipStatus::Active);
                tokio::time::advance(Duration::from_secs(10)).await;
                let (version, next, reviews) = read_notice(&mut peer).await;
                assert_eq!(version, generation);
                assert!(next > sequence);
                sequence = next;
                let mut ids: Vec<_> = reviews
                    .iter()
                    .map(|review| review.conversation_id.as_str())
                    .collect();
                ids.sort_unstable();
                assert_eq!(ids, expected);
                assert_eq!(board.current().0, generation);
                let visibility = board.visibility();
                tokio::time::advance(Duration::from_secs(10)).await;
                assert!(
                    timeout(Duration::from_secs(1), peer.output.recv())
                        .await
                        .is_err(),
                    "unchanged authority revision must not republish the snapshot"
                );
                assert_eq!(board.visibility(), visibility);
            }
            replace_membership(MembershipRole::Member, MembershipStatus::Disabled);
            tokio::time::advance(Duration::from_secs(10)).await;
            let Message::Close(Some(frame)) = peer.message().await else {
                panic!("disabled membership closes the live connection")
            };
            let termination: SessionTermination = serde_json::from_str(&frame.reason).unwrap();
            assert_eq!(termination.code, SessionCloseReason::AuthorizationLost);
            task.await.unwrap();
        }
    }

    #[tokio::test(start_paused = true)]
    async fn review_authority_refresh_wakes_only_its_connection() {
        let (_directory, board) = open_rules();
        let id = "00000000-0000-4000-8000-00000000000a";
        board.waiting(&ReviewNotice {
            conversation_id: id.into(),
            execution_id: ExecutionId::new("turn").unwrap(),
            permission_id: PermissionId::new("first").unwrap(),
            tool: "Bash".into(),
        });
        let (base, authority) = fixture(MembershipRole::Admin);
        let repository = repository_of(vec![owned_conversation(id, "other")]);
        let first_policy = Arc::new(conversation_support::ObservedPolicy::default());
        let second_policy = Arc::new(conversation_support::ObservedPolicy::default());
        let connection = |seconds, policy: Arc<conversation_support::ObservedPolicy>| {
            base.clone()
                .with_permission_rules(board.clone())
                .with_passive_read(
                    Arc::new(UnpairedBinding),
                    repository.clone(),
                    Arc::new(GrantedConversation(id.into())),
                    policy,
                )
                .with_settings(
                    SessionSettings::new(
                        Duration::from_secs(20),
                        Duration::from_secs(5),
                        Duration::from_secs(seconds),
                    )
                    .unwrap(),
                )
        };
        let session = authenticate(&base).await;
        let (first_socket, mut first) = test_socket(None);
        let (second_socket, mut second) = test_socket(None);
        let first_task = tokio::spawn(run_authenticated(
            first_socket,
            connection(10, first_policy.clone()),
            session.clone(),
        ));
        let second_task = tokio::spawn(run_authenticated(
            second_socket,
            connection(30, second_policy.clone()),
            session,
        ));
        let read = async |peer: &mut TestPeer| {
            let Message::Text(text) = peer.message().await else {
                panic!("review event expected")
            };
            let OutgoingMessage::Event(event) = serde_json::from_str(&text).unwrap() else {
                panic!("review event expected")
            };
            assert_eq!(event.event, product_event::PERMISSION_REVIEW_WAITING);
            let payload: PermissionReviewWaiting = serde_json::from_value(event.payload).unwrap();
            (event.state_version, payload.reviews.len())
        };
        assert_eq!(read(&mut first).await, (1, 1));
        assert_eq!(read(&mut second).await, (1, 1));
        assert_eq!((first_policy.asked(), second_policy.asked()), (1, 1));
        let visibility = board.visibility();
        {
            let mut current = authority.snapshot.lock().unwrap();
            let mut next = snapshot(MembershipRole::Member, MembershipStatus::Active);
            next.revision = current.revision + 1;
            *current = next;
        }
        tokio::time::advance(Duration::from_secs(10)).await;
        assert_eq!(read(&mut first).await, (1, 0));
        assert!(
            timeout(Duration::from_secs(1), second.output.recv())
                .await
                .is_err(),
            "one authority refresh must not wake another connection"
        );
        assert_eq!((first_policy.asked(), second_policy.asked()), (2, 1));
        assert_eq!(board.visibility(), visibility);
        // At 21 seconds the fast connection has observed the same revision;
        // the slow connection has not performed its first refresh yet.
        tokio::time::advance(Duration::from_secs(10)).await;
        assert!(timeout(Duration::from_secs(1), first.output.recv())
            .await
            .is_err());
        assert_eq!((first_policy.asked(), second_policy.asked()), (2, 1));
        tokio::time::advance(Duration::from_secs(10)).await;
        assert_eq!(read(&mut second).await, (1, 0));
        assert!(
            timeout(Duration::from_secs(1), first.output.recv())
                .await
                .is_err(),
            "the second refresh must not send a duplicate to the first connection"
        );
        assert_eq!((first_policy.asked(), second_policy.asked()), (2, 2));
        assert_eq!(board.visibility(), visibility);
        board.recheck_visibility();
        assert_eq!(read(&mut first).await, (1, 0));
        assert_eq!(read(&mut second).await, (1, 0));
        assert_eq!((first_policy.asked(), second_policy.asked()), (3, 3));
        board.waiting(&ReviewNotice {
            conversation_id: id.into(),
            execution_id: ExecutionId::new("turn").unwrap(),
            permission_id: PermissionId::new("second").unwrap(),
            tool: "Write".into(),
        });
        assert_eq!(read(&mut first).await, (2, 0));
        assert_eq!(read(&mut second).await, (2, 0));
        assert_eq!((first_policy.asked(), second_policy.asked()), (5, 5));
        first_task.abort();
        second_task.abort();
        let _ = first_task.await;
        let _ = second_task.await;
    }

    #[tokio::test]
    async fn review_authority_updates_survive_initial_and_pending_metadata() {
        for pending_metadata in [false, true] {
            let (_directory, board) = open_rules();
            let id = "00000000-0000-4000-8000-00000000000a";
            board.waiting(&ReviewNotice {
                conversation_id: id.into(),
                execution_id: ExecutionId::new("turn").unwrap(),
                permission_id: PermissionId::new("permission").unwrap(),
                tool: "Bash".into(),
            });
            let (state, authority) = fixture(MembershipRole::Admin);
            let repository = repository_of(vec![owned_conversation(id, "other")]);
            let policy = Arc::new(conversation_support::ObservedPolicy::default());
            let state = state.with_passive_read(
                Arc::new(UnpairedBinding),
                repository.clone(),
                Arc::new(GrantedConversation(id.into())),
                policy.clone(),
            );
            let session = authenticate(&state).await;
            let (authority_send, authority_updates) = watch::channel(session.auth_revision());
            let (entered, mut waiting) = tokio::sync::oneshot::channel();
            let (release, gate) = tokio::sync::oneshot::channel();
            if pending_metadata {
                *repository.metadata_load_gate.lock().unwrap() = Some((entered, gate));
            }
            let (send, mut receive) = mpsc::channel(2);
            let slots = Arc::new(Semaphore::new(2));
            let mut publisher = Box::pin(super::forward_review_notices(
                board,
                send,
                slots,
                state,
                session,
                authority_updates,
            ));
            if pending_metadata {
                assert!(futures_util::poll!(publisher.as_mut()).is_pending());
                waiting.try_recv().expect("the metadata read is suspended");
                assert_eq!(policy.asked(), 0);
            }
            *authority.snapshot.lock().unwrap() =
                snapshot(MembershipRole::Member, MembershipStatus::Active);
            authority.snapshot.lock().unwrap().revision = 2;
            authority_send.send_replace(2);
            if pending_metadata {
                release.send(()).unwrap();
            }
            assert!(futures_util::poll!(publisher.as_mut()).is_pending());
            let passes = if pending_metadata { 2 } else { 1 };
            for _ in 0..passes {
                let queued = drive_review_publication(publisher.as_mut(), &mut receive).await;
                let WireResponse::ReviewWaiting {
                    payload,
                    generation,
                } = queued.message
                else {
                    panic!("review snapshot expected")
                };
                assert!(payload.reviews.is_empty());
                assert_eq!(generation, 1);
            }
            assert_eq!(policy.asked(), passes);
            assert!(receive.try_recv().is_err());
            drop(authority_send);
            assert!(
                futures_util::poll!(publisher.as_mut()).is_ready(),
                "a closed local authority channel must end the publisher"
            );
            assert!(receive.recv().await.is_none());
        }
    }

    #[tokio::test]
    async fn review_authority_updates_survive_waiting_publication() {
        let (_directory, board) = open_rules();
        let id = "00000000-0000-4000-8000-00000000000a";
        board.waiting(&ReviewNotice {
            conversation_id: id.into(),
            execution_id: ExecutionId::new("turn").unwrap(),
            permission_id: PermissionId::new("permission").unwrap(),
            tool: "Bash".into(),
        });
        let (state, authority) = fixture(MembershipRole::Admin);
        let policy = Arc::new(conversation_support::ObservedPolicy::default());
        let audit = Arc::new(conversation_support::RecordingReviewNoticeAudit::default());
        let state = state.with_passive_read(
            Arc::new(UnpairedBinding),
            repository_of(vec![owned_conversation(id, "other")]),
            Arc::new(GrantedConversation(id.into())),
            policy.clone(),
        );
        let state = state.with_review_notice_auditor(Arc::new(ReviewNoticeAuditor::new(
            audit.clone(),
            Arc::new(MonotonicReviewNoticeClock::new()),
        )));
        let session = authenticate(&state).await;
        let (authority_send, authority_updates) = watch::channel(session.auth_revision());
        let (send, mut receive) = mpsc::channel(1);
        let slots = Arc::new(Semaphore::new(1));
        let mut publisher = Box::pin(super::forward_review_notices(
            board,
            send,
            slots,
            state,
            session,
            authority_updates,
        ));
        assert!(futures_util::poll!(publisher.as_mut()).is_pending());
        let first = drive_review_publication(publisher.as_mut(), &mut receive).await;
        assert!(
            matches!(&first.message, WireResponse::ReviewWaiting { payload, .. } if payload.reviews.len() == 1)
        );
        // Retaining the queued frame retains its sole publication permit.
        *authority.snapshot.lock().unwrap() =
            snapshot(MembershipRole::Member, MembershipStatus::Active);
        authority.snapshot.lock().unwrap().revision = 2;
        authority_send.send_replace(2);
        assert!(futures_util::poll!(publisher.as_mut()).is_pending());
        assert_eq!(policy.asked(), 2);
        assert!(receive.try_recv().is_err());
        // The Ready test sink captures and acknowledges in the same task poll.
        // Poll through final version validation into the held UI permit before
        // promotion: this pins admitted in-flight publication, not selection.
        timeout(Duration::from_secs(2), async {
            while audit.records.lock().unwrap().len() < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(futures_util::poll!(publisher.as_mut()).is_pending());
        *authority.snapshot.lock().unwrap() =
            snapshot(MembershipRole::Admin, MembershipStatus::Active);
        authority.snapshot.lock().unwrap().revision = 3;
        authority_send.send_replace(3);
        drop(first);
        assert!(futures_util::poll!(publisher.as_mut()).is_pending());
        let second = drive_review_publication(publisher.as_mut(), &mut receive).await;
        assert!(
            matches!(&second.message, WireResponse::ReviewWaiting { payload, .. } if payload.reviews.is_empty())
        );
        assert_eq!(policy.asked(), 3);
        drop(second);
        assert!(futures_util::poll!(publisher.as_mut()).is_pending());
        let third = drive_review_publication(publisher.as_mut(), &mut receive).await;
        assert!(
            matches!(&third.message, WireResponse::ReviewWaiting { payload, .. } if payload.reviews.len() == 1)
        );
        drop(third);
        drop(authority_send);
        assert!(futures_util::poll!(publisher.as_mut()).is_ready());
    }

    #[tokio::test]
    async fn an_admin_review_notice_obeys_policy_refusals_and_current_membership() {
        struct Refuse(AccessDenial);
        impl ConversationAccessPolicyEvaluator for Refuse {
            fn evaluate(&self, ask: &AccessAsk<'_>) -> AccessDecision {
                assert_eq!(ask.action, AccessAction::ReviewNotice);
                assert!(matches!(
                    ask.subject,
                    AccessSubject::ReviewRecipient {
                        membership: MembershipRole::Admin,
                        ..
                    }
                ));
                AccessDecision::Deny(self.0.clone())
            }
        }
        let (_directory, board) = open_rules();
        let id = "00000000-0000-4000-8000-00000000000a";
        board.waiting(&ReviewNotice {
            conversation_id: id.into(),
            execution_id: ExecutionId::new("turn").unwrap(),
            permission_id: PermissionId::new("permission").unwrap(),
            tool: "Bash".into(),
        });
        let repository = repository_of(vec![owned_conversation(id, "owner")]);
        for refusal in [
            AccessDenial::Unavailable,
            AccessDenial::NotPermitted,
            AccessDenial::Forbidden {
                policies: vec!["test.refusal".into()],
            },
        ] {
            let (state, _) = fixture(MembershipRole::Admin);
            let state = state.with_passive_read(
                Arc::new(UnpairedBinding),
                repository.clone(),
                Arc::new(GrantedConversation(id.into())),
                Arc::new(Refuse(refusal)),
            );
            let session = authenticate(&state).await;
            assert!(notice_snapshot(board.clone(), state, session)
                .await
                .reviews
                .is_empty());
        }
        let (state, authority) = fixture(MembershipRole::Admin);
        let state = state.with_passive_read(
            Arc::new(UnpairedBinding),
            repository,
            Arc::new(GrantedConversation(id.into())),
            conversation_support::access_policy(),
        );
        let session = authenticate(&state).await;
        assert_eq!(
            notice_snapshot(board.clone(), state.clone(), session.clone())
                .await
                .reviews
                .len(),
            1
        );
        *authority.snapshot.lock().unwrap() = snapshot_with(
            MembershipRole::Member,
            MembershipStatus::Active,
            &["server.read"],
        );
        assert!(
            notice_snapshot(board, state, session)
                .await
                .reviews
                .is_empty(),
            "the original admin session supplies current membership, not its old role"
        );
    }

    #[tokio::test]
    async fn review_admission_reads_membership_after_metadata_returns() {
        let (_directory, board) = open_rules();
        let id = "00000000-0000-4000-8000-00000000000a";
        board.waiting(&ReviewNotice {
            conversation_id: id.into(),
            execution_id: ExecutionId::new("turn").unwrap(),
            permission_id: PermissionId::new("permission").unwrap(),
            tool: "Bash".into(),
        });
        for (role, status, visible) in [
            (MembershipRole::Admin, MembershipStatus::Active, true),
            (MembershipRole::Member, MembershipStatus::Active, false),
            (MembershipRole::Admin, MembershipStatus::Disabled, false),
        ] {
            let repository = repository_of(vec![owned_conversation(id, "owner")]);
            let (entered, waiting) = oneshot::channel();
            let (release, released) = oneshot::channel();
            *repository.metadata_load_gate.lock().unwrap() = Some((entered, released));
            let policy = Arc::new(conversation_support::ObservedPolicy::default());
            let (state, authority) = fixture(MembershipRole::Admin);
            let state = state.with_passive_read(
                Arc::new(UnpairedBinding),
                repository,
                Arc::new(GrantedConversation(id.into())),
                policy.clone(),
            );
            let session = authenticate(&state).await;
            let (authority_updates, updates) = watch::channel(session.auth_revision());
            let (send, mut receive) = mpsc::channel(1);
            let task = tokio::spawn(forward_review_notices(
                board.clone(),
                send,
                Arc::new(Semaphore::new(1)),
                state,
                session,
                updates,
            ));
            timeout(Duration::from_secs(2), waiting)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(policy.asked(), 0, "metadata has not reached review policy");
            *authority.snapshot.lock().unwrap() = snapshot_with(role, status, &["server.read"]);
            release.send(()).unwrap();
            if status == MembershipStatus::Disabled {
                assert_eq!(
                    timeout(Duration::from_secs(2), task)
                        .await
                        .unwrap()
                        .unwrap(),
                    Err(ReviewNoticePublicationFailure::Audit(
                        ReviewNoticeAuditFailure::Unavailable
                    ))
                );
                assert!(receive.recv().await.is_none());
                assert_eq!(
                    policy.asked(),
                    0,
                    "disabled membership never reaches policy"
                );
            } else {
                let queued = timeout(Duration::from_secs(2), receive.recv())
                    .await
                    .unwrap()
                    .unwrap();
                let WireResponse::ReviewWaiting {
                    payload: snapshot, ..
                } = queued.message
                else {
                    panic!("expected review snapshot")
                };
                assert_eq!(
                    !snapshot.reviews.is_empty(),
                    visible,
                    "{role:?}, {status:?}"
                );
                drop(authority_updates);
                assert_eq!(
                    timeout(Duration::from_secs(2), task)
                        .await
                        .unwrap()
                        .unwrap(),
                    Ok(())
                );
            }
        }
    }

    /// A repository reply is outside data. Its identity is correlated before
    /// either Cedar or the notice publisher consumes its ownership metadata.
    #[tokio::test]
    async fn a_review_notice_refuses_mismatched_metadata_before_policy_or_publication() {
        let (_directory, board) = open_rules();
        let id_a = "00000000-0000-4000-8000-00000000000a";
        let id_b = "00000000-0000-4000-8000-00000000000b";
        board.waiting(&ReviewNotice {
            conversation_id: id_a.into(),
            execution_id: ExecutionId::new("turn").unwrap(),
            permission_id: PermissionId::new("perm-a").unwrap(),
            tool: "Bash".into(),
        });
        for role in [MembershipRole::Admin, MembershipRole::Member] {
            let repository = repository_of(vec![owned_conversation(id_a, "principal")]);
            *repository.mode_loaded_reply.lock().unwrap() =
                Some(owned_conversation(id_b, "principal"));
            let policy = Arc::new(conversation_support::ObservedPolicy::default());
            let (state, authority) = fixture(role);
            *authority.snapshot.lock().unwrap() =
                snapshot_with(role, MembershipStatus::Active, &["conversation.write"]);
            let state = state.with_passive_read(
                Arc::new(UnpairedBinding),
                repository,
                Arc::new(GrantedConversation(id_a.into())),
                policy.clone(),
            );
            let session = authenticate(&state).await;

            let (_authority_updates, updates) = watch::channel(session.auth_revision());
            let (send, mut receive) = mpsc::channel(1);
            let refused = forward_review_notices(
                board.clone(),
                send,
                Arc::new(Semaphore::new(1)),
                state.clone(),
                session.clone(),
                updates,
            )
            .await;
            assert_eq!(
                refused,
                Err(ReviewNoticePublicationFailure::Audit(
                    ReviewNoticeAuditFailure::Unavailable
                ))
            );
            assert!(receive.recv().await.is_none());
            assert_eq!(policy.asked(), 0, "mismatched metadata reaches no policy");

            let accepted = notice_snapshot(board.clone(), state, session).await;
            assert_eq!(accepted.reviews.len(), 1);
            assert_eq!(accepted.reviews[0].conversation_id, id_a);
            assert_eq!(accepted.reviews[0].tool, "Bash");
            assert!(policy.asked() > 0, "the correlated counterpart asks Cedar");
        }
    }

    /// A paired grant that goes away while the review is still waiting drops
    /// that review from the next snapshot. The board generation does not move.
    #[tokio::test]
    async fn a_revoked_grant_drops_the_waiting_review_from_the_next_snapshot() {
        let (_directory, board) = open_rules();
        let id_a = "00000000-0000-4000-8000-00000000000a";
        let execution = ExecutionId::new("turn").unwrap();
        board.waiting(&ReviewNotice {
            conversation_id: id_a.into(),
            execution_id: execution,
            permission_id: PermissionId::new("perm-a").unwrap(),
            tool: "Bash".into(),
        });
        let open = Arc::new(AtomicBool::new(true));
        let grants = Arc::new(ToggleGrant {
            conversation: id_a.into(),
            open: Arc::clone(&open),
        });
        let (state, authority) = fixture(MembershipRole::Member);
        *authority.snapshot.lock().unwrap() = snapshot_with(
            MembershipRole::Member,
            MembershipStatus::Active,
            &["conversation.read"],
        );
        let state = state.with_passive_read(
            Arc::new(DeviceBinding),
            repository_of(vec![owned_conversation(id_a, "owner")]),
            grants,
            crate::conversation_test_support::access_policy(),
        );
        let session = authenticate(&state).await;
        let (send, mut recv) = mpsc::channel(1);
        let slots = Arc::new(Semaphore::new(2));
        let (_authority, authority_updates) = watch::channel(session.auth_revision());
        let task = tokio::spawn(super::forward_review_notices(
            board.clone(),
            send,
            slots,
            state,
            session,
            authority_updates,
        ));
        let first = tokio::time::timeout(std::time::Duration::from_secs(2), recv.recv())
            .await
            .expect("the granted review is sent")
            .expect("the notice task stays up");
        match first.message {
            WireResponse::ReviewWaiting {
                payload,
                generation,
            } => {
                assert_eq!(payload.reviews.len(), 1);
                assert_eq!(payload.reviews[0].tool, "Bash");
                assert_eq!(generation, 1);
            }
            _ => panic!("expected the board"),
        }
        open.store(false, Ordering::SeqCst);
        let generation_before = board.current().0;
        board.recheck_visibility();
        assert_eq!(board.current().0, generation_before);
        let second = tokio::time::timeout(std::time::Duration::from_secs(2), recv.recv())
            .await
            .expect("the revoked grant is filtered")
            .expect("the notice task stays up");
        match second.message {
            WireResponse::ReviewWaiting {
                payload,
                generation,
            } => {
                assert!(payload.reviews.is_empty());
                assert_eq!(generation, generation_before);
            }
            _ => panic!("expected the filtered board"),
        }
        drop(recv);
        task.abort();
        let _ = task.await;
    }

    /// The five passive reads skip ordinary dispatch, so their Cedar check is
    /// the action `Admit passiveRead` asks for. That action is each method's
    /// manifest grant: a hardcoded grant would fail here once the manifest moved.
    #[tokio::test]
    async fn passive_read_gateway_asks_for_the_manifest_grant() {
        let manifest: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../protocol/product/manifest.json"
        )))
        .unwrap();
        let grants = manifest["methods"].as_object().unwrap();
        let routed = PRODUCT_READY_METHODS
            .iter()
            .copied()
            .filter(|method| matches!(ResponseClass::for_method(method), ResponseClass::Record))
            .collect::<std::collections::BTreeSet<_>>();
        let expected = [
            "conversation.recordsHead",
            "conversation.recordsPage",
            "conversation.catalogueHead",
            "conversation.catalogueManifest",
            "conversation.catalogueResolve",
        ]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(routed, expected);
        let conversation_id = Uuid::new_v4().to_string();
        for method in routed {
            let policy = Arc::new(RecordingGrantPolicy {
                asked: Mutex::new(Vec::new()),
            });
            let (mut state, _) = fixture(MembershipRole::Member);
            state.policy = policy.clone();
            let state = state
                .with_passive_read(
                    Arc::new(RecordBinding),
                    Arc::new(MemoryRepository::default()),
                    Arc::new(crate::conversation_test_support::EveryConversationGranted),
                    crate::conversation_test_support::access_policy(),
                )
                .with_record_source(Arc::new(UnreachableRecordSource))
                .with_catalogue_source(Arc::new(UnreachableCatalogueSource));
            let session = authenticate(&state).await;
            let mut frame = request("grant", method);
            frame.params = passive_read_params(method, &conversation_id);
            let (response, lease) = dispatch_passive_read(
                &state,
                &session,
                frame,
                RecordReadLease::new(()),
                Instant::now() + PASSIVE_READ_TIMEOUT,
            )
            .await;
            assert!(lease.is_none(), "{method}");
            let WireResponse::Ordinary(message) = response else {
                panic!("{method}: ordinary refusal expected");
            };
            let OutgoingMessage::Response(response) = *message else {
                panic!("{method}: refusal expected");
            };
            assert_eq!(response.error.unwrap().code, "forbidden", "{method}");
            let grant = grants[method]["grant"]
                .as_str()
                .unwrap_or_else(|| panic!("{method} has no manifest grant"));
            let asked = policy
                .asked
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone();
            assert_eq!(asked, vec![grant.to_owned()], "{method}");
        }
    }

    fn passive_read_params(method: &str, conversation_id: &str) -> Value {
        let scope = json!({
            "receiver": "receiver",
            "origin": "origin",
            "stream": "stream",
            "incarnation": "incarnation",
            "schema": "schema",
            "accessEpoch": "epoch-3",
        });
        let pass = json!({
            "scope": scope,
            "completed": "0",
            "boundary": "5",
            "generation": "1",
        });
        match method {
            "conversation.recordsHead" => json!({
                "conversationId": conversation_id,
                "receiverId": "receiver",
                "accessEpoch": "3",
            }),
            "conversation.recordsPage" => json!({
                "conversationId": conversation_id,
                "accessEpoch": "3",
                "request": {
                    "scope": scope,
                    "after": "0",
                    "target": "2",
                    "maxRecords": 16,
                    "maxPayloadBytes": MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
                    "maxRecordBytes": MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
                },
            }),
            "conversation.catalogueHead" => json!({
                "receiverId": "receiver",
                "accessEpoch": "3",
            }),
            "conversation.catalogueManifest" => json!({
                "accessEpoch": "3",
                "request": { "pass": pass, "maxEntries": 1 },
            }),
            "conversation.catalogueResolve" => json!({
                "accessEpoch": "3",
                "pass": pass,
                "descriptor": {
                    "key": { "creation": "1", "id": "entry" },
                    "revision": "2",
                    "deleted": false,
                },
                "maxPayloadBytes": 1,
            }),
            _ => panic!("{method} is not a passive read"),
        }
    }

    /// Design row O7: ending an enrollment takes the control lane and its
    /// capacity, so ordinary requests cannot crowd it out; the other pairing
    /// methods are ordinary requests, and every one asks Cedar for
    /// `credential.manage` before it is dispatched.
    #[test]
    fn pairing_deny_and_cancel_are_controls() {
        for method in PRODUCT_READY_METHODS
            .iter()
            .filter(|method| method.starts_with("pairing."))
        {
            let control = matches!(*method, "pairing.deny" | "pairing.cancel");
            assert_eq!(
                matches!(ResponseClass::for_method(method), ResponseClass::Control),
                control,
                "{method}"
            );
            if !control {
                assert!(
                    matches!(ResponseClass::for_method(method), ResponseClass::Ordinary),
                    "{method}"
                );
            }
            assert_eq!(
                action_for_method(method),
                Some("credential.manage"),
                "{method}"
            );
        }
    }

    #[test]
    fn session_termination_is_typed_bounded_and_classifies_authority_failures() {
        for (error, expected, retryable) in [
            (AccessError::CredentialRevoked, "credential_revoked", false),
            (AccessError::CredentialExpired, "credential_expired", false),
            (AccessError::InactiveMembership, "authorization_lost", false),
            (AccessError::Unavailable, "temporary_unavailable", true),
            (AccessError::StaleRevision, "temporary_unavailable", true),
        ] {
            let reason = close_reason(error);
            let value = serde_json::to_value(SessionTermination {
                code: reason,
                retryable: reason.retryable(),
                retry_after_ms: None,
            })
            .unwrap();
            assert_eq!(value["code"], expected);
            assert_eq!(value["retryable"], retryable);
            assert!(value.to_string().len() <= 123);
            assert!(reason.web_socket_code() >= 4000);
        }
    }

    #[test]
    fn record_queue_metadata_stays_within_its_captured_send_fields() {
        let record_fields = std::mem::size_of::<String>() + std::mem::size_of::<Instant>();
        assert!(
            std::mem::size_of::<WireResponse>() <= record_fields + std::mem::size_of::<usize>()
        );
    }

    struct Authority {
        snapshot: Mutex<AccessSnapshot>,
        now: AtomicU64,
        proof_expires_at: Mutex<Option<u64>>,
        /// A share target's principal, by credential id, when it is not the
        /// session's. Absent, every read is the session's actor.
        target_principal: Mutex<std::collections::HashMap<String, PrincipalId>>,
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
                    expires_at: *self.proof_expires_at.lock().unwrap(),
                })
            })
        }
    }

    impl AccessReader for Authority {
        /// The one configured snapshot, answered about the credential asked
        /// for: a share target reads as the session's own actor.
        fn read<'a>(&'a self, credential: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
            Box::pin(async move {
                let mut snapshot = self
                    .snapshot
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone();
                let named = &snapshot.credential;
                let principal = self
                    .target_principal
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .get(credential.as_str())
                    .cloned()
                    .unwrap_or_else(|| named.principal_id().clone());
                if named.id() != credential || principal != *named.principal_id() {
                    snapshot.credential = Credential::new(
                        credential.clone(),
                        principal.clone(),
                        named.organization_id().clone(),
                        named.audience_id().clone(),
                        named.issued_at(),
                        named.expires_at(),
                        named.grants().to_vec(),
                    )
                    .unwrap();
                    let membership = snapshot.membership.clone();
                    snapshot.membership = Membership::new(
                        membership.id().clone(),
                        principal,
                        membership.organization_id().clone(),
                        membership.role(),
                        membership.status(),
                    );
                }
                Ok(snapshot)
            })
        }
    }

    impl Clock for Authority {
        fn unix_milliseconds(&self) -> u64 {
            self.now.load(Ordering::SeqCst) * 1000
        }
    }

    impl UptimeClock for Authority {
        fn elapsed_ms(&self) -> u64 {
            42
        }
    }

    fn snapshot(role: MembershipRole, status: MembershipStatus) -> AccessSnapshot {
        snapshot_with(role, status, &["server.read", "credential.manage"])
    }

    fn snapshot_with(
        role: MembershipRole,
        status: MembershipStatus,
        actions: &[&str],
    ) -> AccessSnapshot {
        let organization = OrganizationId::new("organization").unwrap();
        let resource = Resource::new(
            organization.clone(),
            ResourceId::new("gateway-resource").unwrap(),
        );
        AccessSnapshot {
            credential: Credential::new(
                CredentialId::new("credential").unwrap(),
                PrincipalId::new("principal").unwrap(),
                organization.clone(),
                AudienceId::new("gateway").unwrap(),
                100,
                200,
                actions
                    .iter()
                    .map(|action| Grant::new(Action::new(*action).unwrap(), resource.clone()))
                    .collect(),
            )
            .unwrap(),
            membership: Membership::new(
                MembershipId::new("membership").unwrap(),
                PrincipalId::new("principal").unwrap(),
                organization,
                role,
                status,
            ),
            revision: 1,
        }
    }

    impl nessa_auth::application::pairing::OwnerPhonePurposeSource for Authority {
        fn owner_phone_purpose<'a>(
            &'a self,
            _: &'a AuthenticatedSession,
            _: &'a dyn nessa_auth::application::ports::Clock,
        ) -> nessa_auth::application::ports::PortFuture<
            'a,
            Option<nessa_auth::application::pairing::OwnerPhonePurpose>,
        > {
            // This substitute models ordinary credentials without a pairing ledger.
            Box::pin(async { Ok(None) })
        }
    }

    fn fixture(role: MembershipRole) -> (ProductRouteState, Arc<Authority>) {
        let authority = Arc::new(Authority {
            snapshot: Mutex::new(snapshot(role, MembershipStatus::Active)),
            now: AtomicU64::new(100),
            proof_expires_at: Mutex::new(Some(200)),
            target_principal: Mutex::new(std::collections::HashMap::new()),
        });
        let state = ProductRouteState::new(
            ResourceId::new("gateway-resource").unwrap(),
            OrganizationId::new("organization").unwrap(),
            AudienceId::new("gateway").unwrap(),
            ProductDependencies {
                verifier: authority.clone(),
                owner_phone_purpose: authority.clone(),
                owner_phone_policy: Arc::new(
                    crate::conversation::infrastructure::CedarOwnerPhonePolicyEvaluator::new()
                        .unwrap(),
                ),
                owner_phone_receivers: None,
                access: authority.clone(),
                clock: authority.clone(),
                policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
                uptime_clock: authority.clone(),
                agent_probe: Arc::new(StubAgentProbe::answering(false, false)),
            },
        )
        .with_review_notice_auditor(Arc::new(ReviewNoticeAuditor::new(
            Arc::new(conversation_support::RecordingReviewNoticeAudit::default()),
            Arc::new(MonotonicReviewNoticeClock::new()),
        )));
        (state, authority)
    }

    async fn authenticate(state: &ProductRouteState) -> AuthenticatedSession {
        AuthenticateSession {
            verifier: state.verifier.as_ref(),
            access: state.access.as_ref(),
            clock: state.clock.as_ref(),
        }
        .execute(
            &CredentialEvidence::new(b"secret".to_vec()).unwrap(),
            state.audience(),
        )
        .await
        .unwrap()
    }

    fn request(id: &str, method: &str) -> RequestFrame {
        RequestFrame {
            kind: "req".into(),
            id: id.into(),
            method: method.into(),
            params: json!({}),
        }
    }

    struct RejectingAdmin(CredentialAdminError);
    impl CredentialAdmin for RejectingAdmin {
        fn issue<'a>(
            &'a self,
            _: IssueCredentialRequest,
        ) -> PortFuture<'a, IssueCredentialOutcome, CredentialAdminError> {
            Box::pin(async { Err(self.0) })
        }
        fn list<'a>(
            &'a self,
            _: ListCredentialsRequest,
        ) -> PortFuture<'a, Vec<CredentialMetadataDto>, CredentialAdminError> {
            Box::pin(async { Err(self.0) })
        }
        fn revoke<'a>(
            &'a self,
            _: RevokeCredentialRequest,
        ) -> PortFuture<'a, RevokeCredentialOutcome, CredentialAdminError> {
            Box::pin(async { Err(self.0) })
        }
    }

    fn issue_params() -> Value {
        json!({
            "requestId": "issue",
            "principal": {"id": "reader", "kind": "integration"},
            "membership": {"id": "reader", "principalId": "reader",
                "organizationId": "organization", "role": "member", "state": "active"},
            "grants": [{"action": "server.read",
                "resource": {"organizationId": "organization", "id": "gateway-resource"}}]
        })
    }

    #[tokio::test]
    async fn administration_reports_typed_rejections_from_a_substitute_adapter() {
        for (error, code) in [
            (CredentialAdminError::Conflict, "credential_conflict"),
            (CredentialAdminError::Capacity, "credential_capacity"),
            (CredentialAdminError::NotFound, "credential_not_found"),
            (
                CredentialAdminError::Unavailable,
                "credential_store_unavailable",
            ),
        ] {
            let (state, _) = fixture(MembershipRole::Admin);
            let state = state.with_admin(Arc::new(RejectingAdmin(error)));
            let session = authenticate(&state).await;
            for (method, params) in [
                ("credential.issue", issue_params()),
                ("credential.list", json!({})),
                (
                    "credential.revoke",
                    json!({"requestId": "revoke", "credentialId": "credential"}),
                ),
            ] {
                let mut frame = request("command", method);
                frame.params = params;
                let OutgoingMessage::Response(response) = dispatch(&state, &session, frame).await
                else {
                    panic!("response expected")
                };
                assert_eq!(response.error.unwrap().code, code);
            }
        }
    }

    #[tokio::test]
    async fn empty_grants_are_rejected_before_calling_administration() {
        let (state, _) = fixture(MembershipRole::Admin);
        let state = state.with_admin(Arc::new(RejectingAdmin(CredentialAdminError::Unavailable)));
        let session = authenticate(&state).await;
        let mut frame = request("empty", "credential.issue");
        frame.params = issue_params();
        frame.params["grants"] = json!([]);
        let OutgoingMessage::Response(response) = dispatch(&state, &session, frame).await else {
            panic!("response expected")
        };
        assert_eq!(response.error.unwrap().code, "invalid_request");
    }

    #[tokio::test]
    async fn passive_read_grant_cannot_call_agent_or_mutation_routes() {
        let (state, authority) = fixture(MembershipRole::Member);
        {
            let mut snapshot = authority.snapshot.lock().unwrap();
            let organization = snapshot.credential.organization_id().clone();
            snapshot.credential = Credential::new(
                CredentialId::new("credential").unwrap(),
                PrincipalId::new("principal").unwrap(),
                organization.clone(),
                AudienceId::new("gateway").unwrap(),
                100,
                200,
                vec![Grant::new(
                    Action::new("conversation.read").unwrap(),
                    Resource::new(organization, ResourceId::new("gateway-resource").unwrap()),
                )],
            )
            .unwrap();
        }
        let session = authenticate(&state).await;
        for method in [
            "conversation.read",
            "conversation.list",
            "conversation.observe",
            "conversation.send",
            "conversation.answer",
            "conversation.answerQuestion",
            "conversation.archive",
            "conversation.delete",
        ] {
            let OutgoingMessage::Response(response) =
                dispatch(&state, &session, request("read-only", method)).await
            else {
                panic!("response expected");
            };
            assert_eq!(response.error.unwrap().code, "forbidden", "{method}");
        }
    }

    #[tokio::test]
    async fn admitted_record_route_uses_owner_scope_and_real_physical_source() {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("conversations");
        nessa_local_storage::create_directory(&private).unwrap();
        let conversations = Arc::new(
            LocalConversationStore::open(&private.join("metadata.sqlite3"))
                .unwrap()
                .with_access_policy(crate::conversation_test_support::access_policy()),
        );
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        conversations
            .create(
                Conversation::new(
                    id.clone(),
                    OrganizationId::new("organization").unwrap(),
                    PrincipalId::new("principal").unwrap(),
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
        let storage = Arc::new(RecordStorage::new(directory.path().join("records")).unwrap());
        storage.initialize().await.unwrap();
        let session_id = SessionId::new(id.to_string()).unwrap();
        let writer = storage.open(session_id.clone()).await.unwrap();
        let provider = ProviderIdentity::new("fixture", "model", "workspace").unwrap();
        writer
            .save_changes(
                writer.load().await.unwrap().binding().clone(),
                vec![SessionSaveUnit::new(vec![SessionChange::Opened {
                    id: session_id.clone(),
                    provider,
                    context: ProviderContext::Absent,
                }])
                .unwrap()],
            )
            .await
            .unwrap();
        let origin = Id::new("gateway-resource").unwrap();
        let identity = storage
            .record_identity(&session_id, origin.clone())
            .await
            .unwrap()
            .unwrap();
        let sid = |value: &str| Id::new(value).unwrap();
        let scope = identity.scope(sid("receiver"), sid("epoch-3"));
        let scope_json = serde_json::json!({
            "receiver": scope.receiver().as_str(),
            "origin": scope.origin().as_str(),
            "stream": scope.stream().as_str(),
            "incarnation": scope.incarnation().as_str(),
            "schema": scope.schema().as_str(),
            "accessEpoch": scope.access_epoch().as_str(),
        });
        let (state, authority) = fixture(MembershipRole::Member);
        {
            let mut snapshot = authority.snapshot.lock().unwrap();
            snapshot.credential = Credential::new(
                CredentialId::new("credential").unwrap(),
                PrincipalId::new("principal").unwrap(),
                OrganizationId::new("organization").unwrap(),
                AudienceId::new("gateway").unwrap(),
                100,
                200,
                vec![Grant::new(
                    Action::new("conversation.read").unwrap(),
                    Resource::new(
                        OrganizationId::new("organization").unwrap(),
                        ResourceId::new("gateway-resource").unwrap(),
                    ),
                )],
            )
            .unwrap();
        }
        let source = Arc::new(NessaRecordReadSource::new(
            storage.clone(),
            origin,
            Handle::current(),
        ));
        let state = state
            .with_passive_read(
                Arc::new(RecordBinding),
                conversations,
                Arc::new(crate::conversation_test_support::EveryConversationGranted),
                crate::conversation_test_support::access_policy(),
            )
            .with_record_source(source.clone());
        let session = authenticate(&state).await;
        let mut head_frame = request("head", "conversation.recordsHead");
        head_frame.params = serde_json::json!({
            "conversationId": id.to_string(), "accessEpoch": "3", "receiverId": "receiver",
        });
        let (head_wire, lease) = dispatch_passive_read(
            &state,
            &session,
            head_frame,
            RecordReadLease::new(()),
            Instant::now() + PASSIVE_READ_TIMEOUT,
        )
        .await;
        let WireResponse::Record { text, .. } = head_wire else {
            panic!("record reply expected")
        };
        let head_json: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(head_json["payload"]["head"], "2");
        assert_eq!(head_json["payload"]["scope"]["accessEpoch"], "epoch-3");
        drop(lease);
        for target in ["1", "2"] {
            let mut page_frame = request("page", "conversation.recordsPage");
            page_frame.params = serde_json::json!({
                "conversationId": id.to_string(), "accessEpoch": "3",
                "request": {
                    "scope": scope_json.clone(), "after": "0", "target": target, "maxRecords": 16,
                    "maxPayloadBytes": SDK_MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
                    "maxRecordBytes": SDK_MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
                },
            });
            let (page_wire, lease) = dispatch_passive_read(
                &state,
                &session,
                page_frame,
                RecordReadLease::new(()),
                Instant::now() + PASSIVE_READ_TIMEOUT,
            )
            .await;
            if target == "1" {
                let WireResponse::Ordinary(message) = page_wire else {
                    panic!("intermediate Unit target must refuse")
                };
                let OutgoingMessage::Response(failure) = *message else {
                    panic!("refusal expected")
                };
                assert_eq!(failure.error.unwrap().code, "invalid_request");
            } else {
                let WireResponse::Record { text, .. } = page_wire else {
                    panic!("record reply expected")
                };
                let page_json: Value = serde_json::from_str(&text).unwrap();
                assert_eq!(page_json["payload"]["records"].as_array().unwrap().len(), 2);
                assert_eq!(page_json["payload"]["records"][0]["position"], "1");
                assert_eq!(page_json["payload"]["records"][1]["position"], "2");
                assert_eq!(page_json["payload"]["request"]["target"], "2");
                assert_eq!(
                    page_json["payload"]["request"]["scope"]["accessEpoch"],
                    "epoch-3"
                );
            }
            drop(lease);
        }
        let mut wrong = request("wrong", "conversation.recordsHead");
        wrong.params = serde_json::json!({
            "conversationId": id.to_string(), "accessEpoch": "3",
            "receiverId": "wrong",
        });
        let (failure, _) = dispatch_passive_read(
            &state,
            &session,
            wrong,
            RecordReadLease::new(()),
            Instant::now() + PASSIVE_READ_TIMEOUT,
        )
        .await;
        let WireResponse::Ordinary(message) = failure else {
            panic!("ordinary refusal expected")
        };
        let OutgoingMessage::Response(failure) = *message else {
            panic!("refusal expected")
        };
        assert_eq!(failure.error.unwrap().code, "wrong_receiver");
        source.shutdown().await.unwrap();
        drop(writer);
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn health_uses_real_cedar_and_current_membership() {
        let (state, authority) = fixture(MembershipRole::Member);
        let session = authenticate(&state).await;
        let OutgoingMessage::Response(allowed) =
            dispatch(&state, &session, request("1", "server.health")).await
        else {
            panic!("response expected")
        };
        assert!(allowed.ok);
        assert_eq!(allowed.payload.unwrap()["uptimeMs"], 42);

        *authority
            .snapshot
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            snapshot(MembershipRole::Member, MembershipStatus::Disabled);
        assert_eq!(
            current_session_error(&state, &session).await,
            Some(AccessError::InactiveMembership)
        );
    }

    #[tokio::test]
    async fn member_cannot_reach_credential_handler_even_with_grant() {
        let (state, _) = fixture(MembershipRole::Member);
        let session = authenticate(&state).await;
        let OutgoingMessage::Response(denied) =
            dispatch(&state, &session, request("2", "credential.list")).await
        else {
            panic!("response expected")
        };
        assert!(!denied.ok);
        assert_eq!(denied.error.unwrap().code, "forbidden");
    }

    /// What the MCP server methods answer through `dispatch` on `state` for
    /// `method` with `params`: whether it succeeded, and its payload, or its
    /// error code and details.
    async fn mcp_servers_call(
        state: &ProductRouteState,
        session: &AuthenticatedSession,
        method: &str,
        params: Value,
    ) -> (bool, Value) {
        let frame = RequestFrame {
            params,
            ..request("mcp", method)
        };
        let OutgoingMessage::Response(response) = dispatch(state, session, frame).await else {
            panic!("response expected")
        };
        if response.ok {
            (true, response.payload.unwrap())
        } else {
            let error = response.error.unwrap();
            (false, json!({"code": error.code, "details": error.details}))
        }
    }

    /// #391 S1: a caller without `credential.manage` is refused `forbidden`
    /// by every MCP server method before its params are read, and nothing is
    /// audited, read or written.
    #[tokio::test]
    async fn s1_mcp_servers_are_forbidden_without_credential_manage_before_params() {
        use crate::mcp_servers::infrastructure::settings_test_support::{
            config, settings_for, MemoryFiles, RecordingAudit,
        };
        let files = MemoryFiles::holding(config(vec![]));
        let audit = Arc::new(RecordingAudit::default());
        let (settings, _) = settings_for(files.clone(), audit.clone());
        let (state, _) = fixture(MembershipRole::Member);
        let state = state.with_mcp_server_settings(Arc::new(settings));
        let session = authenticate(&state).await;
        for method in [
            "mcpServers.list",
            "mcpServers.save",
            "mcpServers.remove",
            "mcpServers.inspect",
            "mcpServers.authorize",
            "mcpServers.revoke",
        ] {
            assert_eq!(action_for_method(method), Some("credential.manage"));
            let (ok, answer) =
                mcp_servers_call(&state, &session, method, json!({"not": "params"})).await;
            assert!(!ok);
            assert_eq!(answer["code"], "forbidden", "{method}");
        }
        assert!(audit.records().is_empty());
        assert_eq!(files.locks.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(files.publishes.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    struct HooksClock;
    impl nessa_auth::application::ports::Clock for HooksClock {
        fn unix_milliseconds(&self) -> u64 {
            1
        }
    }

    struct AgentCaller;
    impl crate::agents::application::CallerKindSource for AgentCaller {
        fn caller_kind(&self, _: &str) -> &'static str {
            "agent"
        }
    }

    /// A member who can write conversations is refused before params are read,
    /// and nothing is written. The credential also carries `conversation.write`,
    /// so a method mapped to that grant would pass this caller.
    #[tokio::test]
    async fn review_setting_native_hooks_needs_the_owner_grant() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let preferences = crate::agents::infrastructure::NativeHooksPreferences::open(
            directory.clone(),
            Arc::new(HooksClock),
        )
        .unwrap();
        let (state, authority) = fixture(MembershipRole::Member);
        *authority
            .snapshot
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = snapshot_with(
            MembershipRole::Member,
            MembershipStatus::Active,
            &["server.read", "conversation.write", "credential.manage"],
        );
        let state = state.with_native_hooks(preferences);
        let session = authenticate(&state).await;
        assert_eq!(
            action_for_method("agents.setNativeHooks"),
            Some("credential.manage")
        );
        let (ok, answer) = mcp_servers_call(
            &state,
            &session,
            "agents.setNativeHooks",
            json!({"not": "params"}),
        )
        .await;
        assert!(!ok);
        assert_eq!(answer["code"], "forbidden");
        assert!(!directory.join("native-hooks.json").exists());
        let audit = directory.join("audit").join("native-hooks");
        let records = std::fs::read_dir(&audit)
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .ok()
                    .and_then(|entry| entry.path().extension().map(|ext| ext == "json"))
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(records, 0);
    }

    #[tokio::test]
    async fn an_agent_caller_is_recorded_as_an_agent() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        let preferences = crate::agents::infrastructure::NativeHooksPreferences::open(
            directory.clone(),
            Arc::new(HooksClock),
        )
        .unwrap();
        let (state, _) = fixture(MembershipRole::Admin);
        let state = state
            .with_native_hooks(preferences)
            .with_caller_kinds(Arc::new(AgentCaller));
        let session = authenticate(&state).await;
        let (ok, _) = mcp_servers_call(
            &state,
            &session,
            "agents.setNativeHooks",
            json!({"requestId": "turn", "agent": "claude", "enabled": true}),
        )
        .await;
        assert!(ok);
        let recorded = std::fs::read_dir(directory.join("audit").join("native-hooks"))
            .unwrap()
            .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
            .find(|text| text.contains("\"phase\":\"requested\""))
            .unwrap();
        let value: Value = serde_json::from_str(&recorded).unwrap();
        assert_eq!(value["initiator"]["kind"], "agent");
    }

    /// The MCP server methods on the wire: `kind: "stdio"`, variable names in
    /// a list and never a value, a null value keeping the stored one, and each
    /// refusal typed with its details; a gateway with no live set to manage
    /// answers `mcp_servers_not_configured`.
    #[tokio::test]
    async fn mcp_servers_on_the_wire_carry_names_only_and_typed_refusals() {
        use crate::mcp_servers::infrastructure::settings_test_support::{
            absolute, config, settings_for, MemoryFiles, RecordingAudit,
        };
        let (bare, _) = fixture(MembershipRole::Admin);
        let session = authenticate(&bare).await;
        assert_eq!(
            mcp_servers_call(&bare, &session, "mcpServers.list", json!({})).await,
            (
                false,
                json!({"code": "mcp_servers_not_configured", "details": null})
            )
        );
        let files = MemoryFiles::holding(config(vec![]));
        let audit = Arc::new(RecordingAudit::default());
        let (settings, _) = settings_for(files.clone(), audit.clone());
        let (state, _) = fixture(MembershipRole::Admin);
        let state = state.with_mcp_server_settings(Arc::new(settings));
        let session = authenticate(&state).await;
        let list = |state: &ProductRouteState| {
            let state = state.clone();
            let session = session.clone();
            async move {
                let (ok, listed) =
                    mcp_servers_call(&state, &session, "mcpServers.list", json!({})).await;
                assert!(ok, "{listed}");
                listed
            }
        };
        let listed = list(&state).await;
        assert_eq!(listed["servers"][0]["kind"], "stdio");
        assert_eq!(listed["servers"][0]["managed"], true);
        let command = absolute("/usr/bin/python3");
        let input = |env: Value| {
            json!({"kind": "stdio", "name": "mcptest", "command": command,
                "args": ["/s.py"], "env": env, "enabled": true})
        };
        let (ok, saved) = mcp_servers_call(
            &state,
            &session,
            "mcpServers.save",
            json!({"revision": listed["revision"],
                "server": input(json!([{"name": "API_TOKEN", "value": "secret-value"}]))}),
        )
        .await;
        assert!(ok, "{saved}");
        assert_eq!(saved["live"], true, "{saved}");
        let listed = list(&state).await;
        assert_eq!(listed["revision"], saved["revision"]);
        assert_eq!(
            listed["servers"][0],
            json!({"kind": "stdio", "name": "mcptest", "command": command,
                "args": ["/s.py"], "envNames": ["API_TOKEN"], "enabled": true, "managed": false})
        );
        assert!(!listed.to_string().contains("secret-value"));
        // A null keeps the stored value.
        let (ok, kept) = mcp_servers_call(
            &state,
            &session,
            "mcpServers.save",
            json!({"revision": listed["revision"],
                "server": input(json!([{"name": "API_TOKEN", "value": null}]))}),
        )
        .await;
        assert!(ok, "{kept}");
        assert_eq!(
            files.document()["agents"]["mcpServers"][0]["env"],
            json!({"API_TOKEN": "secret-value"})
        );
        let refusals = [
            (
                "mcpServers.save",
                json!({"revision": "stale", "server": input(json!([]))}),
                json!({"code": "mcp_servers_revision_conflict",
                    "details": {"revision": kept["revision"]}}),
            ),
            (
                "mcpServers.save",
                json!({"revision": kept["revision"],
                    "server": input(json!([{"name": "NEVER", "value": null}]))}),
                json!({"code": "mcp_servers_invalid",
                    "details": {"problem": "environment_value_missing", "server": "mcptest",
                        "name": "NEVER"}}),
            ),
            (
                "mcpServers.remove",
                json!({"revision": kept["revision"], "name": "nessa"}),
                json!({"code": "mcp_servers_reserved_name", "details": null}),
            ),
            (
                "mcpServers.remove",
                json!({"revision": kept["revision"], "name": "unknown"}),
                json!({"code": "mcp_servers_not_found", "details": null}),
            ),
            (
                "mcpServers.save",
                json!({"revision": kept["revision"],
                    "server": {"kind": "remote", "name": "x", "command": "/x", "args": [],
                        "env": [], "enabled": true}}),
                json!({"code": "invalid_request", "details": null}),
            ),
            (
                "mcpServers.list",
                json!({"extra": true}),
                json!({"code": "invalid_request", "details": null}),
            ),
            // A variable without `value` is refused, never read as "keep"
            // (pass 2b, decision 6).
            (
                "mcpServers.save",
                json!({"revision": kept["revision"],
                    "server": input(json!([{"name": "API_TOKEN"}]))}),
                json!({"code": "invalid_request", "details": null}),
            ),
            // A bad variable name is named.
            (
                "mcpServers.save",
                json!({"revision": kept["revision"],
                    "server": input(json!([{"name": "1BAD", "value": "v"}]))}),
                json!({"code": "mcp_servers_invalid",
                    "details": {"problem": "environment_name", "server": "mcptest",
                        "name": "1BAD"}}),
            ),
        ];
        for (method, params, expected) in refusals {
            assert_eq!(
                mcp_servers_call(&state, &session, method, params.clone()).await,
                (false, expected),
                "{method} {params}"
            );
        }
        // The configuration not published: nothing applied. Published, but
        // its directory not synced: applied, and said so.
        files
            .fail_publish
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            mcp_servers_call(
                &state,
                &session,
                "mcpServers.save",
                json!({"revision": kept["revision"], "server": input(json!([]))}),
            )
            .await,
            (
                false,
                json!({"code": "mcp_servers_storage_unavailable",
                    "details": {"applied": false}})
            )
        );
        files
            .fail_publish
            .store(false, std::sync::atomic::Ordering::SeqCst);
        files
            .fail_sync
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            mcp_servers_call(
                &state,
                &session,
                "mcpServers.save",
                json!({"revision": kept["revision"],
                    "server": input(json!([{"name": "API_TOKEN", "value": null}]))}),
            )
            .await,
            (
                false,
                json!({"code": "mcp_servers_storage_unavailable",
                    "details": {"applied": true}})
            )
        );
        files
            .fail_sync
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let kept = list(&state).await;
        audit
            .fail_outcome
            .store(true, std::sync::atomic::Ordering::SeqCst);
        // A refusal whose outcome cannot be recorded carries the refusal's
        // code (pass 2b, decision 4).
        assert_eq!(
            mcp_servers_call(
                &state,
                &session,
                "mcpServers.remove",
                json!({"revision": kept["revision"], "name": "unknown"}),
            )
            .await,
            (
                false,
                json!({"code": "audit_unavailable",
                    "details": {"applied": false, "code": "mcp_servers_not_found"}})
            )
        );
        assert_eq!(
            mcp_servers_call(
                &state,
                &session,
                "mcpServers.remove",
                json!({"revision": kept["revision"], "name": "mcptest"}),
            )
            .await,
            (
                false,
                json!({"code": "audit_unavailable", "details": {"applied": true}})
            )
        );
        // An entry added to the file by hand that breaks a rule is named,
        // whichever server the save was about.
        let mut hand_added = files.document();
        hand_added["agents"]["mcpServers"] = json!([
            {"name": "hand-added", "command": "relative/server", "args": []}
        ]);
        *files.bytes.lock().unwrap() = Some(serde_json::to_vec(&hand_added).unwrap());
        audit
            .fail_outcome
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let listed = list(&state).await;
        assert_eq!(
            mcp_servers_call(
                &state,
                &session,
                "mcpServers.save",
                json!({"revision": listed["revision"], "server": input(json!([]))}),
            )
            .await,
            (
                false,
                json!({"code": "mcp_servers_invalid",
                    "details": {"problem": "command", "server": "hand-added"}})
            )
        );
        // Once shutdown has begun, a change is not admitted.
        state.close_mcp_server_admission();
        assert_eq!(
            mcp_servers_call(
                &state,
                &session,
                "mcpServers.remove",
                json!({"revision": listed["revision"], "name": "hand-added"}),
            )
            .await,
            (
                false,
                json!({"code": "mcp_servers_stopping", "details": null})
            )
        );
    }

    /// `mcpServers.inspect` on the wire: the tools with their hints and each
    /// app's CSP and permissions in `mcp.readResource`'s shapes; each failure
    /// typed, a server's JSON-RPC error with its details; an inspection cut
    /// by shutdown; and a stored name required.
    #[tokio::test]
    async fn mcp_servers_inspect_answers_typed_tools_and_typed_failures() {
        use crate::mcp_servers::application::{
            InspectCut, InspectFailure, InspectedTool, InspectedUi, Inspection, ServerProblem,
        };
        use crate::mcp_servers::infrastructure::settings_test_support::{
            config, entry, inspected_over, LeapingClock, MemoryFiles, RecordingAudit,
            ScriptedInspector,
        };
        use nessa_sdk::domain::mcp_apps::{UiCsp, UiPermissions};
        let inspector = Arc::new(ScriptedInspector::default());
        let (settings, _) = inspected_over(
            MemoryFiles::holding(config(vec![entry("a")])),
            Arc::new(RecordingAudit::default()),
            Arc::new(LeapingClock::default()),
            inspector.clone(),
        );
        let (state, _) = fixture(MembershipRole::Admin);
        let state = state.with_mcp_server_settings(Arc::new(settings));
        let session = authenticate(&state).await;
        *inspector.answer.lock().unwrap() = Ok(Inspection {
            tools: vec![
                InspectedTool {
                    name: "chart".into(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    ui: Some(InspectedUi {
                        uri: "ui://a/chart.html".into(),
                        csp: UiCsp::new(
                            vec!["https://api.example.com".into()],
                            vec![],
                            vec![],
                            vec![],
                        )
                        .unwrap(),
                        permissions: UiPermissions {
                            camera: true,
                            ..UiPermissions::default()
                        },
                    }),
                },
                InspectedTool {
                    name: "plain".into(),
                    read_only_hint: None,
                    destructive_hint: None,
                    ui: None,
                },
            ],
            cut: Some(InspectCut::Tools),
        });
        let inspect = |name: &str| {
            let state = state.clone();
            let session = session.clone();
            let params = json!({"name": name});
            async move { mcp_servers_call(&state, &session, "mcpServers.inspect", params).await }
        };
        assert_eq!(
            inspect("a").await,
            (
                true,
                json!({"complete": false, "cut": "tools", "tools": [
                    {"name": "chart", "readOnlyHint": true, "destructiveHint": false,
                        "ui": {"uri": "ui://a/chart.html",
                            "csp": {"connectDomains": ["https://api.example.com"],
                                "resourceDomains": [], "frameDomains": [], "baseUriDomains": []},
                            "permissions": {"camera": true, "microphone": false,
                                "geolocation": false, "clipboardWrite": false}}},
                    {"name": "plain"},
                ]})
            )
        );
        for (failure, expected) in [
            (
                InspectFailure::StartFailed,
                json!({"code": "mcp_server_start_failed", "details": null}),
            ),
            (
                InspectFailure::TimedOut,
                json!({"code": "mcp_server_timed_out", "details": null}),
            ),
            (
                InspectFailure::Gone,
                json!({"code": "mcp_server_gone", "details": null}),
            ),
            (
                InspectFailure::Malformed,
                json!({"code": "mcp_server_malformed", "details": null}),
            ),
            (
                InspectFailure::Stopping,
                json!({"code": "mcp_servers_stopping", "details": null}),
            ),
            // A stored server the SDK refuses to start: its problem, named.
            (
                InspectFailure::Invalid(ServerProblem::EnvironmentName {
                    server: "a".into(),
                    name: "1BAD".into(),
                }),
                json!({"code": "mcp_servers_invalid",
                    "details": {"problem": "environment_name", "server": "a", "name": "1BAD"}}),
            ),
            (
                InspectFailure::RemoteError {
                    code: -32002,
                    message: "no\nsuch".into(),
                },
                json!({"code": "mcp_server_remote_error",
                    "details": {"code": -32002, "message": "no such"}}),
            ),
            (
                InspectFailure::RemoteError {
                    code: i64::MAX,
                    message: "far".into(),
                },
                json!({"code": "mcp_server_remote_error", "details": null}),
            ),
        ] {
            *inspector.answer.lock().unwrap() = Err(failure);
            assert_eq!(inspect("a").await, (false, expected));
        }
        // Cut by shutdown after the server was started: incomplete, no tools.
        *inspector.answer.lock().unwrap() = Ok(Inspection {
            tools: vec![],
            cut: Some(InspectCut::Stopping),
        });
        assert_eq!(
            inspect("a").await,
            (
                true,
                json!({"complete": false, "cut": "stopping", "tools": []})
            )
        );
        assert_eq!(
            inspect("unknown").await,
            (
                false,
                json!({"code": "mcp_servers_not_found", "details": null})
            )
        );
        assert_eq!(
            mcp_servers_call(&state, &session, "mcpServers.inspect", json!({})).await,
            (false, json!({"code": "invalid_request", "details": null}))
        );
    }

    /// I5: an inspection whose answer would pass the frame's 64 KiB loses
    /// tools from the end until it fits, and says so with `cut: "bytes"` —
    /// for the longest request id too; one already cut keeps its own cut.
    #[tokio::test]
    async fn i5_an_answer_past_the_frame_bound_drops_tools_until_it_fits() {
        use crate::mcp_servers::application::{InspectCut, InspectedTool, InspectedUi, Inspection};
        use nessa_sdk::domain::mcp_apps::{UiCsp, UiPermissions};
        let source = |n: usize| format!("https://{}.example.com", "d".repeat(400 + n % 7));
        let tool = |n: usize| InspectedTool {
            name: format!("tool_{n}"),
            read_only_hint: Some(true),
            destructive_hint: None,
            ui: Some(InspectedUi {
                uri: format!("ui://a/{n}.html"),
                csp: UiCsp::new((0..8).map(source).collect(), vec![], vec![], vec![]).unwrap(),
                permissions: UiPermissions::default(),
            }),
        };
        let request_id = "\u{1}".repeat(256);
        for (cut, said) in [(None, "bytes"), (Some(InspectCut::Ui), "ui")] {
            let inspection = Inspection {
                tools: (0..64).map(tool).collect(),
                cut,
            };
            let message = super::super::mcp_servers::fitted(&request_id, inspection);
            let text = message.to_wire_text().unwrap();
            assert!(text.len() <= MAX_PAYLOAD_BYTES as usize, "{}", text.len());
            let OutgoingMessage::Response(response) = message else {
                panic!("a response")
            };
            let payload = response.payload.unwrap();
            assert_eq!(payload["complete"], false);
            assert_eq!(payload["cut"], said);
            let kept = payload["tools"].as_array().unwrap();
            assert!(!kept.is_empty() && kept.len() < 64, "{}", kept.len());
            // From the end: the first ones stay, in order.
            for (index, each) in kept.iter().enumerate() {
                assert_eq!(each["name"], format!("tool_{index}"));
            }
        }
        // One that fits is answered whole and complete.
        let small = Inspection {
            tools: (0..2).map(tool).collect(),
            cut: None,
        };
        let OutgoingMessage::Response(response) = super::super::mcp_servers::fitted("id", small)
        else {
            panic!("a response")
        };
        let payload = response.payload.unwrap();
        assert_eq!(payload["complete"], true);
        assert!(payload.get("cut").is_none());
        assert_eq!(payload["tools"].as_array().unwrap().len(), 2);
    }

    /// I5 at the edge: an answer of exactly the frame's 65536 bytes is sent
    /// whole and complete; one byte more loses its last tool and says
    /// `cut: "bytes"`.
    #[test]
    fn i5_the_frame_bound_holds_at_exactly_its_edge() {
        use crate::mcp_servers::application::{InspectedTool, Inspection};
        let limit = MAX_PAYLOAD_BYTES as usize;
        let tool = |name: String| InspectedTool {
            name,
            read_only_hint: None,
            destructive_hint: None,
            ui: None,
        };
        let answered = |padding: usize| {
            let inspection = Inspection {
                tools: vec![tool("first".into()), tool("p".repeat(padding))],
                cut: None,
            };
            let message = super::super::mcp_servers::fitted("id", inspection);
            let length = message.to_wire_text().unwrap().len();
            let OutgoingMessage::Response(response) = message else {
                panic!("a response")
            };
            (length, response.payload.unwrap())
        };
        // The frame grows a byte for each byte of an ASCII name.
        let (small, _) = answered(1);
        let edge = 1 + limit - small;
        let (length, payload) = answered(edge);
        assert_eq!(length, limit);
        assert_eq!(payload["complete"], true);
        assert_eq!(payload["tools"].as_array().unwrap().len(), 2);
        let (length, payload) = answered(edge + 1);
        assert!(length <= limit, "{length}");
        assert_eq!(payload["complete"], false);
        assert_eq!(payload["cut"], "bytes");
        assert_eq!(payload["tools"].as_array().unwrap().len(), 1);
        assert_eq!(payload["tools"][0]["name"], "first");
    }

    /// One listed server whose single argument is `padding` bytes long.
    fn padded_list(padding: usize) -> crate::mcp_servers::application::ServerList {
        use crate::mcp_servers::application::{ListedServer, ServerList};
        use crate::mcp_servers::domain::StdioServer;
        ServerList {
            revision: "r".repeat(64),
            servers: vec![ListedServer {
                remote_id: None,
                url: None,
                server: StdioServer::new("s", "/usr/bin/python3", vec!["p".repeat(padding)]),
                env_names: vec![],
                enabled: true,
                managed: false,
            }],
        }
    }

    /// L1 and L2 at the edge: a list answer of exactly the frame's 65536
    /// bytes is sent whole; one byte more is refused
    /// `mcp_servers_config_too_large` with the revision, in a frame that fits.
    #[test]
    fn l2_a_list_past_the_frame_bound_is_refused_with_its_revision() {
        let limit = MAX_PAYLOAD_BYTES as usize;
        let answered = |padding: usize| {
            let message = super::super::mcp_servers::answered("id", padded_list(padding));
            let length = message.to_wire_text().unwrap().len();
            let OutgoingMessage::Response(response) = message else {
                panic!("a response")
            };
            (length, response)
        };
        // The frame grows a byte for each byte of an ASCII argument.
        let (small, _) = answered(1);
        let edge = 1 + limit - small;
        let (length, response) = answered(edge);
        assert_eq!(length, limit);
        assert!(response.ok);
        assert_eq!(
            response.payload.unwrap()["servers"][0]["args"][0]
                .as_str()
                .unwrap()
                .len(),
            edge
        );
        let (length, response) = answered(edge + 1);
        assert!(length <= limit, "{length}");
        assert!(!response.ok);
        let error = response.error.unwrap();
        assert_eq!(error.code, "mcp_servers_config_too_large");
        assert_eq!(error.details, Some(json!({"revision": "r".repeat(64)})));
    }

    /// W1 at the edge: a save's list is measured for the longest request
    /// id — 256 bytes, each written as six — so a list that passed answers
    /// whole whatever id later asks for it.
    #[cfg(unix)]
    #[test]
    fn w1_a_list_fits_for_the_longest_request_id_at_exactly_its_edge() {
        use super::super::mcp_servers::{answered, list_fits};
        let limit = MAX_PAYLOAD_BYTES as usize;
        let longest = "\u{1}".repeat(256);
        let length = |padding: usize| {
            answered(&longest, padded_list(padding))
                .to_wire_text()
                .unwrap()
                .len()
        };
        let edge = 1 + limit - length(1);
        assert_eq!(length(edge), limit);
        assert!(list_fits(&padded_list(edge)));
        assert!(!list_fits(&padded_list(edge + 1)));
        // With a short id the same list has room to spare.
        assert!(answered("id", padded_list(edge + 1)).is_success());
    }

    /// A remove answers whether the stored list is now live: `false` while a
    /// list edited by hand is still past the count after it (its server
    /// taken out of the live set all the same), `true` once it is within it.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_remove_answers_whether_the_list_went_live() {
        use crate::mcp_servers::infrastructure::settings_test_support::{
            config, entry, settings_for, MemoryFiles, RecordingAudit,
        };
        let files = MemoryFiles::holding(config(
            (0..17)
                .map(|index| entry(&format!("s{index:02}")))
                .collect(),
        ));
        let (settings, _) = settings_for(files, Arc::new(RecordingAudit::default()));
        let (state, _) = fixture(MembershipRole::Admin);
        let state = state.with_mcp_server_settings(Arc::new(settings));
        let session = authenticate(&state).await;
        let (_, listed) = mcp_servers_call(&state, &session, "mcpServers.list", json!({})).await;
        let mut revision = listed["revision"].clone();
        // With the managed server, 17 and then 16: past the count, then within it.
        for (name, live) in [("s16", false), ("s15", true)] {
            let (ok, removed) = mcp_servers_call(
                &state,
                &session,
                "mcpServers.remove",
                json!({"revision": revision, "name": name}),
            )
            .await;
            assert!(ok, "{removed}");
            assert_eq!(removed["live"], live, "{name}: {removed}");
            revision = removed["revision"].clone();
        }
    }

    /// L2, W1 and W2 over a store bounded as the gateway's: a file edited by
    /// hand whose list would not fit is listed as a refusal carrying its
    /// revision, and a remove naming that revision is made — even one that
    /// leaves the list too long still — so removes recover; a save whose
    /// list would not fit is refused `mcp_servers_config_too_large`, nothing
    /// written, its outcome recorded refused.
    #[cfg(unix)]
    #[tokio::test]
    async fn w1_a_save_whose_list_would_not_fit_is_refused_and_a_remove_recovers() {
        use crate::mcp_servers::application::{McpServerAuditPhase, McpServerOutcome};
        use crate::mcp_servers::infrastructure::settings_test_support::{
            absolute, config, settings_at_full_size, MemoryFiles, RecordingAudit,
        };
        // s0 small, s1 to s14 padded.
        let stored = |padding: usize| {
            (0..15)
                .map(|index| {
                    let arg = if index == 0 {
                        "/s0.py".to_owned()
                    } else {
                        "p".repeat(padding)
                    };
                    json!({"name": format!("s{index}"),
                        "command": absolute("/usr/bin/python3"), "args": [arg]})
                })
                .collect::<Vec<_>>()
        };
        // Just under the file's 64 KiB, which the listed fields outgrow.
        let bare = serde_json::to_vec(&config(stored(0))).unwrap().len();
        let padding = (65_000 - bare) / 14;
        let document = config(stored(padding));
        assert!(serde_json::to_vec(&document).unwrap().len() <= 65_536);
        let files = MemoryFiles::holding(document);
        let audit = Arc::new(RecordingAudit::default());
        let (settings, _) = settings_at_full_size(files.clone(), audit.clone());
        let (state, _) = fixture(MembershipRole::Admin);
        let state = state.with_mcp_server_settings(Arc::new(settings));
        let session = authenticate(&state).await;
        let (ok, refused) = mcp_servers_call(&state, &session, "mcpServers.list", json!({})).await;
        assert!(!ok);
        assert_eq!(refused["code"], "mcp_servers_config_too_large");
        let revision = refused["details"]["revision"].clone();
        assert!(revision.is_string(), "{refused}");
        // Removing the small one leaves a list still too long: made all the
        // same.
        let (ok, removed) = mcp_servers_call(
            &state,
            &session,
            "mcpServers.remove",
            json!({"revision": revision, "name": "s0"}),
        )
        .await;
        assert!(ok, "{removed}");
        let (ok, refused) = mcp_servers_call(&state, &session, "mcpServers.list", json!({})).await;
        assert!(!ok);
        assert_eq!(
            refused,
            json!({"code": "mcp_servers_config_too_large",
                "details": {"revision": removed["revision"]}})
        );
        let (ok, removed) = mcp_servers_call(
            &state,
            &session,
            "mcpServers.remove",
            json!({"revision": removed["revision"], "name": "s1"}),
        )
        .await;
        assert!(ok, "{removed}");
        let (ok, listed) = mcp_servers_call(&state, &session, "mcpServers.list", json!({})).await;
        assert!(ok, "{listed}");
        assert_eq!(listed["revision"], removed["revision"]);
        assert_eq!(listed["servers"].as_array().unwrap().len(), 14);
        // Saving it back would leave a list past the frame, though the file
        // would fit.
        let old = files.current();
        let (ok, answer) = mcp_servers_call(
            &state,
            &session,
            "mcpServers.save",
            json!({"revision": listed["revision"], "server": {"kind": "stdio",
                "name": "s1", "command": absolute("/usr/bin/python3"),
                "args": ["p".repeat(padding)], "env": [], "enabled": true}}),
        )
        .await;
        assert_eq!(
            (ok, answer),
            (
                false,
                json!({"code": "mcp_servers_config_too_large", "details": null})
            )
        );
        assert_eq!(files.current(), old);
        let records = audit.records();
        assert!(matches!(
            &records.last().unwrap().phase,
            McpServerAuditPhase::Outcome(McpServerOutcome::Refused {
                reason: "config_too_large",
                ..
            })
        ));
    }

    struct RecordingGrantPolicy {
        asked: Mutex<Vec<String>>,
    }

    impl PolicyEvaluator for RecordingGrantPolicy {
        fn evaluate(
            &self,
            _: &AuthContext,
            action: &Action,
            _: &Resource,
            _: &AccessSnapshot,
        ) -> Result<Decision, AccessError> {
            self.asked
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(action.as_str().to_owned());
            Ok(Decision::Deny)
        }
    }

    struct UnreachableCatalogueSource;

    impl CatalogueReadSource for UnreachableCatalogueSource {
        fn read(
            &self,
            _: crate::conversation::application::AdmittedCatalogueRead,
            _: PrincipalId,
            _: CatalogueReadOperation,
            _: RecordReadLease,
        ) -> CatalogueReadFuture<'_> {
            panic!("grant check must refuse before catalogue source admission");
        }
    }

    struct UnavailablePolicy;
    impl PolicyEvaluator for UnavailablePolicy {
        fn evaluate(
            &self,
            _: &AuthContext,
            _: &Action,
            _: &Resource,
            _: &AccessSnapshot,
        ) -> Result<Decision, AccessError> {
            Err(AccessError::Unavailable)
        }
    }

    struct UnavailableAccess;
    impl AccessReader for UnavailableAccess {
        fn read<'a>(&'a self, _: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
            Box::pin(async { Err(AccessError::Unavailable) })
        }
    }

    struct PendingRecordAuthority;
    impl AccessReader for PendingRecordAuthority {
        fn read<'a>(&'a self, _: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
            Box::pin(std::future::pending())
        }
    }

    struct RecoveringRecordAuthority {
        authority: Arc<Authority>,
        first_failure: AccessError,
        reads: AtomicU64,
    }
    impl AccessReader for RecoveringRecordAuthority {
        fn read<'a>(&'a self, id: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
            Box::pin(async move {
                if self.reads.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(self.first_failure)
                } else {
                    self.authority.read(id).await
                }
            })
        }
    }

    #[tokio::test(start_paused = true)]
    async fn passive_socket_preserves_retryable_authority_failures() {
        for (error, expected) in [
            (AccessError::Denied, "forbidden"),
            (AccessError::Unavailable, "unverifiable"),
            (AccessError::StaleRevision, "unverifiable"),
            (AccessError::InvalidCredential, "unauthorized"),
            (AccessError::CredentialRevoked, "unauthorized"),
            (AccessError::CredentialExpired, "unauthorized"),
            (AccessError::InactiveMembership, "unauthorized"),
            (AccessError::IdentityMismatch, "unauthorized"),
            (AccessError::Unsupported, "unverifiable"),
        ] {
            for method in [
                "conversation.recordsHead",
                "conversation.recordsPage",
                "conversation.catalogueHead",
                "conversation.catalogueManifest",
                "conversation.catalogueResolve",
            ] {
                let (mut state, authority) = fixture(MembershipRole::Member);
                let session = authenticate(&state).await;
                state.access = Arc::new(RecoveringRecordAuthority {
                    authority,
                    first_failure: error,
                    reads: AtomicU64::new(0),
                });
                state.record_source = Some(Arc::new(UnreachableRecordSource));
                state.settings = SessionSettings::new(
                    Duration::from_secs(20),
                    Duration::from_secs(5),
                    Duration::from_secs(30),
                )
                .unwrap();
                let permits = state.record_reads.clone();
                let available = permits.available_permits();
                let (socket, mut peer) = test_socket(None);
                let task = tokio::spawn(run_authenticated(socket, state, session));
                let mut responses = Vec::new();
                for id in ["outage", "recovered"] {
                    peer.input
                        .send(Ok(Message::Text(
                            json!({"type":"req", "id":id, "method":method, "params":{}})
                                .to_string()
                                .into(),
                        )))
                        .unwrap();
                    responses.push(timeout(Duration::from_secs(1), peer.output.recv()).await);
                }
                drop(peer.input);
                timeout(Duration::from_secs(1), task)
                    .await
                    .unwrap()
                    .unwrap();
                for (received, id, code) in [
                    (responses.remove(0), "outage", expected),
                    (responses.remove(0), "recovered", "invalid_request"),
                ] {
                    let Message::Text(text) = received.unwrap().unwrap() else {
                        panic!("correlated read refusal expected")
                    };
                    let value: Value = serde_json::from_str(&text).unwrap();
                    assert_eq!(value["id"], id);
                    assert_eq!(value["error"]["code"], code, "{error:?}, {method}");
                }
                assert_eq!(permits.available_permits(), available);
            }
        }
    }

    struct CountingRecordAuthority {
        authority: Arc<Authority>,
        reads: AtomicU64,
        first_read_delay: Duration,
    }
    impl AccessReader for CountingRecordAuthority {
        fn read<'a>(&'a self, id: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
            Box::pin(async move {
                let read = self.reads.fetch_add(1, Ordering::SeqCst);
                if read == 0 && !self.first_read_delay.is_zero() {
                    tokio::time::sleep(self.first_read_delay).await;
                }
                self.authority.read(id).await
            })
        }
    }

    struct UnreachableRecordSource;
    impl RecordReadSource for UnreachableRecordSource {
        fn read<'a>(
            &'a self,
            _: ReceiverReadScope,
            _: RecordReadOperation,
            _: RecordReadLease,
        ) -> RecordReadFuture<'a, RecordReadResponse> {
            panic!("pending authority must not reach record metadata/source");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn record_phase_deadline_includes_current_authority_before_source_admission() {
        for before_dispatch in [Duration::ZERO, Duration::from_secs(9)] {
            let (mut state, _) = fixture(MembershipRole::Member);
            let session = authenticate(&state).await;
            state.access = Arc::new(PendingRecordAuthority);
            state.record_source = Some(Arc::new(UnreachableRecordSource));
            let permits = Arc::new(Semaphore::new(1));
            let lease = RecordReadLease::new(permits.clone().try_acquire_owned().unwrap());
            let start = Instant::now();
            tokio::time::advance(before_dispatch).await;
            let (response, lease) = dispatch_passive_read(
                &state,
                &session,
                request("deadline", "conversation.recordsHead"),
                lease,
                start + PASSIVE_READ_TIMEOUT,
            )
            .await;
            assert_eq!(Instant::now() - start, PASSIVE_READ_TIMEOUT);
            assert!(lease.is_none());
            assert_eq!(permits.available_permits(), 1);
            let WireResponse::Ordinary(message) = response else {
                panic!("ordinary refusal expected")
            };
            let OutgoingMessage::Response(response) = *message else {
                panic!("timeout refusal expected")
            };
            assert_eq!(response.error.unwrap().code, "read_timeout");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn socket_passive_read_uses_one_deadline_for_slow_authority() {
        for method in [
            "conversation.recordsHead",
            "conversation.recordsPage",
            "conversation.catalogueHead",
            "conversation.catalogueManifest",
            "conversation.catalogueResolve",
        ] {
            let (mut state, _) = fixture(MembershipRole::Member);
            let session = authenticate(&state).await;
            state.access = Arc::new(PendingRecordAuthority);
            state.record_source = Some(Arc::new(UnreachableRecordSource));
            // Isolate frame admission from the separate periodic socket check.
            state.settings = SessionSettings::new(
                Duration::from_secs(20),
                Duration::from_secs(5),
                Duration::from_secs(30),
            )
            .unwrap();
            let permits = state.record_reads.clone();
            let available = permits.available_permits();
            let (socket, mut peer) = test_socket(None);
            let start = Instant::now();
            let task = tokio::spawn(run_authenticated(socket, state, session));
            peer.input
                .send(Ok(Message::Text(
                    json!({
                        "type": "req", "id": "deadline", "method": method, "params": {}
                    })
                    .to_string()
                    .into(),
                )))
                .unwrap();
            let result = timeout(Duration::from_secs(11), peer.output.recv()).await;
            task.abort();
            let _ = task.await;
            let Message::Text(text) = result
                .expect("passive refusal must arrive within its read budget")
                .expect("socket must return a refusal")
            else {
                panic!("correlated read refusal expected")
            };
            let value: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value["id"], "deadline");
            assert_eq!(value["error"]["code"], "read_timeout");
            assert_eq!(Instant::now() - start, PASSIVE_READ_TIMEOUT);
            assert_eq!(permits.available_permits(), available);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn expired_passive_read_does_not_poll_ready_authority() {
        for method in [
            "conversation.recordsHead",
            "conversation.recordsPage",
            "conversation.catalogueHead",
            "conversation.catalogueManifest",
            "conversation.catalogueResolve",
        ] {
            for (elapsed, code, reads) in [
                (9, "invalid_request", 1),
                (10, "read_timeout", 0),
                (11, "read_timeout", 0),
            ] {
                let (mut state, authority) = fixture(MembershipRole::Member);
                let session = authenticate(&state).await;
                let authority = Arc::new(CountingRecordAuthority {
                    authority,
                    reads: AtomicU64::new(0),
                    first_read_delay: Duration::ZERO,
                });
                state.access = authority.clone();
                state.record_source = Some(Arc::new(UnreachableRecordSource));
                let permits = Arc::new(Semaphore::new(1));
                let lease = RecordReadLease::new(permits.clone().try_acquire_owned().unwrap());
                let deadline = Instant::now() + PASSIVE_READ_TIMEOUT;
                tokio::time::advance(Duration::from_secs(elapsed)).await;
                let (response, lease) = dispatch_passive_read(
                    &state,
                    &session,
                    request("expired", method),
                    lease,
                    deadline,
                )
                .await;
                let WireResponse::Ordinary(message) = response else {
                    panic!("ordinary refusal expected")
                };
                let OutgoingMessage::Response(response) = *message else {
                    panic!("correlated refusal expected")
                };
                assert_eq!(response.id, "expired");
                assert_eq!(response.error.unwrap().code, code);
                assert_eq!(authority.reads.load(Ordering::SeqCst), reads);
                assert!(lease.is_none());
                assert_eq!(permits.available_permits(), 1);
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn passive_read_ready_result_at_expiry_is_a_timeout() {
        let (mut state, authority) = fixture(MembershipRole::Member);
        let session = authenticate(&state).await;
        let authority = Arc::new(CountingRecordAuthority {
            authority,
            reads: AtomicU64::new(0),
            first_read_delay: PASSIVE_READ_TIMEOUT,
        });
        state.access = authority.clone();
        state.record_source = Some(Arc::new(UnreachableRecordSource));
        let permits = Arc::new(Semaphore::new(1));
        let lease = RecordReadLease::new(permits.clone().try_acquire_owned().unwrap());
        let start = Instant::now();
        let (response, lease) = dispatch_passive_read(
            &state,
            &session,
            request("ready", "conversation.recordsHead"),
            lease,
            start + PASSIVE_READ_TIMEOUT,
        )
        .await;
        let WireResponse::Ordinary(message) = response else {
            panic!("ordinary refusal expected")
        };
        let OutgoingMessage::Response(response) = *message else {
            panic!("correlated timeout expected")
        };
        assert_eq!(response.id, "ready");
        assert_eq!(response.error.unwrap().code, "read_timeout");
        assert_eq!(Instant::now() - start, PASSIVE_READ_TIMEOUT);
        assert_eq!(authority.reads.load(Ordering::SeqCst), 1);
        assert!(lease.is_none());
        assert_eq!(permits.available_permits(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn authority_ready_at_read_deadline_does_not_admit_source_work() {
        for delay in [0, 9, 10, 11] {
            let (state, authority) = fixture(MembershipRole::Member);
            authority.snapshot.lock().unwrap().credential = Credential::new(
                CredentialId::new("credential").unwrap(),
                PrincipalId::new("principal").unwrap(),
                OrganizationId::new("organization").unwrap(),
                AudienceId::new("gateway").unwrap(),
                100,
                200,
                vec![Grant::new(
                    Action::new("conversation.read").unwrap(),
                    Resource::new(
                        OrganizationId::new("organization").unwrap(),
                        ResourceId::new("gateway-resource").unwrap(),
                    ),
                )],
            )
            .unwrap();
            let session = authenticate(&state).await;
            let authority = Arc::new(CountingRecordAuthority {
                authority,
                reads: AtomicU64::new(0),
                first_read_delay: Duration::from_secs(delay),
            });
            let repository = Arc::new(MemoryRepository::default());
            let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
            repository.records.lock().unwrap().insert(
                id.clone(),
                Conversation::new(
                    id.clone(),
                    OrganizationId::new("organization").unwrap(),
                    PrincipalId::new("principal").unwrap(),
                    "panel".into(),
                    "create".into(),
                    1,
                    AgentId::Claude,
                    ConversationModelId::new("model").unwrap(),
                    ConversationApprovalMode::Ask,
                )
                .unwrap(),
            );
            let source = Arc::new(RecordAdmissionSpy(AtomicU64::new(0)));
            let mut state = state
                .with_passive_read(
                    Arc::new(RecordBinding),
                    repository,
                    Arc::new(crate::conversation_test_support::EveryConversationGranted),
                    crate::conversation_test_support::access_policy(),
                )
                .with_record_source(source.clone());
            state.access = authority;
            let permits = Arc::new(Semaphore::new(1));
            let lease = RecordReadLease::new(permits.clone().try_acquire_owned().unwrap());
            let mut frame = request("admission", "conversation.recordsHead");
            frame.params =
                json!({"conversationId":id.to_string(),"accessEpoch":"3","receiverId":"receiver"});
            let start = Instant::now();
            let (response, lease) =
                dispatch_passive_read(&state, &session, frame, lease, start + PASSIVE_READ_TIMEOUT)
                    .await;
            let WireResponse::Ordinary(message) = response else {
                panic!("ordinary refusal expected")
            };
            let OutgoingMessage::Response(response) = *message else {
                panic!("correlated refusal expected")
            };
            assert_eq!(response.id, "admission");
            assert_eq!(
                response.error.unwrap().code,
                if delay < 10 {
                    "temporarily_unavailable"
                } else {
                    "read_timeout"
                }
            );
            assert_eq!(
                source.0.load(Ordering::SeqCst),
                u64::from(delay < 10),
                "authority delay {delay}"
            );
            assert_eq!(Instant::now() - start, Duration::from_secs(delay.min(10)));
            assert!(lease.is_none());
            assert_eq!(permits.available_permits(), 1);
        }
    }

    struct MustNotRun;
    impl UptimeClock for MustNotRun {
        fn elapsed_ms(&self) -> u64 {
            panic!("unauthorized request reached the health handler")
        }
    }

    #[tokio::test]
    async fn invalid_current_authority_never_reaches_a_handler() {
        for attack in [
            "revoked", "expired", "disabled", "stale", "identity", "store", "policy", "tenant",
        ] {
            let (mut state, authority) = fixture(MembershipRole::Member);
            let session = authenticate(&state).await;
            state.uptime_clock = Arc::new(MustNotRun);
            match attack {
                "revoked" => authority
                    .snapshot
                    .lock()
                    .unwrap()
                    .credential
                    .restore_revoked_at(100)
                    .unwrap(),
                "expired" => authority.now.store(200, Ordering::SeqCst),
                "disabled" => {
                    *authority.snapshot.lock().unwrap() =
                        snapshot(MembershipRole::Member, MembershipStatus::Disabled)
                }
                "stale" => authority.snapshot.lock().unwrap().revision = 0,
                "identity" => {
                    authority.snapshot.lock().unwrap().membership = Membership::new(
                        MembershipId::new("other-membership").unwrap(),
                        PrincipalId::new("principal").unwrap(),
                        OrganizationId::new("organization").unwrap(),
                        MembershipRole::Member,
                        MembershipStatus::Active,
                    );
                }
                "store" => state.access = Arc::new(UnavailableAccess),
                "policy" => state.policy = Arc::new(UnavailablePolicy),
                "tenant" => {
                    state.gateway = Resource::new(
                        OrganizationId::new("other-organization").unwrap(),
                        ResourceId::new("gateway-resource").unwrap(),
                    )
                }
                _ => unreachable!(),
            }
            let OutgoingMessage::Response(response) =
                dispatch(&state, &session, request("attack", "server.health")).await
            else {
                panic!("response expected")
            };
            assert!(!response.ok, "allowed {attack}");
            assert!(response.payload.is_none());
        }
    }

    #[tokio::test]
    async fn health_rejects_caller_supplied_identity_and_invalid_envelopes() {
        let (mut state, _) = fixture(MembershipRole::Member);
        let session = authenticate(&state).await;
        state.uptime_clock = Arc::new(MustNotRun);
        let mut forged = request("forged", "server.health");
        forged.params = json!({"principalId": "owner", "role": "admin"});
        for frame in [
            forged,
            request("", "server.health"),
            request("unknown", "conversation.unknown"),
        ] {
            assert!(!dispatch(&state, &session, frame).await.is_success());
        }
    }

    #[test]
    fn malformed_correlatable_request_gets_invalid_request() {
        let response = correlatable_invalid_request(
            r#"{"type":"req","id":"request-7","method":"server.health","params":{},"extra":true}"#,
        )
        .expect("request id is recoverable");
        let OutgoingMessage::Response(response) = response else {
            panic!("response expected")
        };
        assert_eq!(response.id, "request-7");
        assert_eq!(response.error.unwrap().code, "invalid_request");
        assert!(correlatable_invalid_request("not json").is_none());
    }

    #[test]
    fn a_nested_lone_surrogate_is_answered_invalid_request() {
        for (method, id_first) in [
            ("mcp.callTool", true),
            ("mcp.callTool", false),
            ("server.health", true),
            ("server.health", false),
        ] {
            let params = r#"{"nested":"\ud800"}"#;
            let text = if id_first {
                format!(
                    r#"{{"type":"req","id":"request-9","method":"{method}","params":{params}}}"#
                )
            } else {
                format!(
                    r#"{{"type":"req","method":"{method}","params":{params},"id":"request-9"}}"#
                )
            };
            assert!(
                RequestFrame::decode(&text).is_err(),
                "{method} decoded; the refusal path was not reached"
            );
            let Some(OutgoingMessage::Response(response)) = correlatable_invalid_request(&text)
            else {
                panic!("{method} id_first={id_first} got no answer")
            };
            assert_eq!(response.id, "request-9");
            assert!(!response.ok);
            assert_eq!(response.error.unwrap().code, "invalid_request");
        }
        for (method, params) in [
            ("mcp.callTool", r#"{"a":"\ud800\x"}"#),
            ("server.health", r#"{"a":"\udfff\x"}"#),
        ] {
            let text = format!(
                r#"{{"type":"req","method":"{method}","params":{params},"id":"request-9"}}"#
            );
            assert!(RequestFrame::decode(&text).is_err());
            let Some(OutgoingMessage::Response(response)) = correlatable_invalid_request(&text)
            else {
                panic!("{method} with a byte after the surrogate got no answer")
            };
            assert_eq!(response.id, "request-9");
            assert_eq!(response.error.unwrap().code, "invalid_request");
        }
        assert!(correlatable_invalid_request(
            r#"{"type":"req","id":"\ud800","method":"server.health","params":{}}"#
        )
        .is_none());
        assert!(correlatable_invalid_request(
            r#"{"type":"req","id":"a","id":"b","method":"mcp.callTool","params":{"nested":"\ud800"}}"#
        )
        .is_none());
        assert!(correlatable_invalid_request(
            r#"{"type":"req","id":"request-9","method":"a","method":"b","params":{"a":"\ud800"}}"#
        )
        .is_none());
        assert!(correlatable_invalid_request(
            r#"{"type":"req","id":"","method":"m","params":{"a":"\ud800"}}"#
        )
        .is_none());
        let long_id = "x".repeat(257);
        assert!(correlatable_invalid_request(&format!(
            r#"{{"type":"req","id":"{long_id}","method":"m","params":{{"a":"\ud800"}}}}"#
        ))
        .is_none());
        // 128 é is 256 bytes and is answered. 200 é is 200 code points and 400
        // bytes, so it is not: the table counts UTF-8 bytes.
        let exact = "é".repeat(128);
        let Some(OutgoingMessage::Response(response)) = correlatable_invalid_request(&format!(
            r#"{{"type":"req","id":"{exact}","method":"m","params":{{"a":"\ud800"}}}}"#
        )) else {
            panic!("256-byte id got no answer");
        };
        assert_eq!(response.id, exact);
        let wide = "é".repeat(200);
        assert!(correlatable_invalid_request(&format!(
            r#"{{"type":"req","id":"{wide}","method":"m","params":{{"a":"\ud800"}}}}"#
        ))
        .is_none());
        assert!(correlatable_invalid_request(
            r#"{"type":"event","id":"request-9","payload":{"a":"\ud800"}}"#
        )
        .is_none());
        let Some(OutgoingMessage::Response(response)) = correlatable_invalid_request(
            r#"{"type":"req","id":"request-9","method":"m","params":{},"\ud800":1,"\ud800":2}"#,
        ) else {
            panic!("two non-Unicode names hid the id");
        };
        assert_eq!(response.id, "request-9");
        assert_eq!(response.error.unwrap().code, "invalid_request");
        let Some(OutgoingMessage::Response(response)) = correlatable_invalid_request(
            r#"{"type":"req","id":"request-9","method":"m","params":{"p":1,"p":2}}"#,
        ) else {
            panic!("a repeated nested name hid the id");
        };
        assert_eq!(response.id, "request-9");
        let mut nested = "0".to_string();
        for _ in 0..127 {
            nested = format!("[{nested}]");
        }
        assert!(correlatable_invalid_request(&format!(
            r#"{{"type":"req","id":"request-9","method":"m","params":{nested}}}"#
        ))
        .is_none());
    }

    #[tokio::test]
    async fn session_ready_reports_current_restrictions_and_registered_methods() {
        let (state, _) = fixture(MembershipRole::Member);
        let session = authenticate(&state).await;
        let snapshot = current_snapshot(&state, &session).await.unwrap();
        let value = serde_json::to_value(session_ready(&state, &session, &snapshot)).unwrap();
        assert!(
            wire_shape_product_session_ready(&value),
            "producer readiness must satisfy its published schema: {} methods",
            value["methods"].as_array().unwrap().len()
        );
        assert_eq!(value["grants"].as_array().unwrap().len(), 2);
        assert!(value["methods"]
            .as_array()
            .unwrap()
            .iter()
            .any(|method| method == "credential.revoke"));
    }

    /// Deterministic socket backpressure: messages become visible only when
    /// flush is released. Exercises the production session loop without relying
    /// on OS socket-buffer sizes or flooding a real network connection.
    struct TestSocket {
        incoming: tokio::sync::mpsc::UnboundedReceiver<Result<Message, Error>>,
        outgoing: tokio::sync::mpsc::UnboundedSender<Message>,
        writing: tokio::sync::mpsc::UnboundedSender<()>,
        gate: Option<tokio::sync::oneshot::Receiver<()>>,
        pending: Vec<Message>,
    }
    impl Stream for TestSocket {
        type Item = Result<Message, Error>;
        fn poll_next(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            self.incoming.poll_recv(cx)
        }
    }
    impl Sink<Message> for TestSocket {
        type Error = std::io::Error;
        fn poll_ready(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn start_send(
            mut self: std::pin::Pin<&mut Self>,
            message: Message,
        ) -> Result<(), Self::Error> {
            self.pending.push(message);
            let _ = self.writing.send(());
            Ok(())
        }
        fn poll_flush(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            if let Some(gate) = &mut self.gate {
                if std::future::Future::poll(std::pin::Pin::new(gate), cx).is_pending() {
                    return std::task::Poll::Pending;
                }
                self.gate = None;
            }
            for message in std::mem::take(&mut self.pending) {
                self.outgoing
                    .send(message)
                    .map_err(|_| std::io::Error::other("test peer closed"))?;
            }
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_close(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            self.poll_flush(cx)
        }
    }
    struct TestPeer {
        input: tokio::sync::mpsc::UnboundedSender<Result<Message, Error>>,
        output: tokio::sync::mpsc::UnboundedReceiver<Message>,
        writing: tokio::sync::mpsc::UnboundedReceiver<()>,
    }
    fn test_socket(gate: Option<tokio::sync::oneshot::Receiver<()>>) -> (TestSocket, TestPeer) {
        let (input, incoming) = tokio::sync::mpsc::unbounded_channel();
        let (outgoing, output) = tokio::sync::mpsc::unbounded_channel();
        let (writing, writes) = tokio::sync::mpsc::unbounded_channel();
        (
            TestSocket {
                incoming,
                outgoing,
                writing,
                gate,
                pending: vec![],
            },
            TestPeer {
                input,
                output,
                writing: writes,
            },
        )
    }
    impl TestPeer {
        fn request(&self, id: &str) {
            self.input
                .send(Ok(Message::Text(
                    serde_json::to_string(
                        &json!({"type": "req", "id": id, "method": "server.health", "params": {}}),
                    )
                    .unwrap()
                    .into(),
                )))
                .unwrap();
        }
        async fn message(&mut self) -> Message {
            timeout(Duration::from_secs(1), self.output.recv())
                .await
                .unwrap()
                .unwrap()
        }
    }
    #[tokio::test]
    async fn queued_control_precedes_ordinary_after_a_stalled_write() {
        let (release, gate) = tokio::sync::oneshot::channel();
        let (socket, mut peer) = test_socket(Some(gate));
        let (sink, _incoming) = socket.split();
        let (controls_send, controls) = mpsc::channel(4);
        let (refusals_send, refusals) = mpsc::channel(1);
        let (ordinary_send, ordinary) = mpsc::channel(16);
        let (record_send, records) = mpsc::channel(1);
        let slots = Arc::new(Semaphore::new(3));
        let response = |id: &str, slot: OwnedSemaphorePermit| QueuedResponse {
            message: WireResponse::ordinary(success(id, &json!({}))),
            _slot: Arc::new(slot),
            _record_work: None,
            _mount: None,
        };
        ordinary_send
            .send(response(
                "first",
                slots.clone().try_acquire_owned().unwrap(),
            ))
            .await
            .unwrap();
        let subscriptions = Arc::new(SubscriptionDeliveries::new(Default::default()));
        let writer = tokio::spawn(write_authenticated(
            sink,
            controls,
            no_close(),
            refusals,
            ordinary,
            records,
            Duration::from_secs(1),
            Arc::default(),
            subscriptions.clone(),
        ));
        peer.writing.recv().await.unwrap();
        assert_eq!(slots.available_permits(), 2, "in-flight send owns its slot");
        ordinary_send
            .send(response(
                "ordinary",
                slots.clone().try_acquire_owned().unwrap(),
            ))
            .await
            .unwrap();
        controls_send
            .send(ControlOutput::Response(Box::new(response(
                "control",
                slots.clone().try_acquire_owned().unwrap(),
            ))))
            .await
            .unwrap();
        release.send(()).unwrap();
        for expected in ["first", "control", "ordinary"] {
            let Message::Text(text) = peer.message().await else {
                panic!("response expected")
            };
            let value: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value["id"], expected);
        }
        // Close the same delivery interest as the production connection owner.
        subscriptions.close();
        drop(controls_send);
        drop(ordinary_send);
        drop(refusals_send);
        drop(record_send);
        writer.await.unwrap();
        assert_eq!(slots.available_permits(), 3);
    }

    #[tokio::test]
    async fn maximum_record_frame_keeps_same_socket_control_live_without_raising_ordinary_cap() {
        let escaped = "\u{0001}".repeat(128);
        let scope = RecordScope {
            receiver: escaped.clone(),
            origin: escaped.clone(),
            stream: escaped.clone(),
            incarnation: escaped.clone(),
            schema: escaped.clone(),
            access_epoch: escaped.clone(),
        };
        let record = encode_response(
            &"\u{0001}".repeat(256),
            &ConversationRecordsPageResult {
                request: RecordPageRequest {
                    scope,
                    after: u64::MAX.to_string(),
                    target: u64::MAX.to_string(),
                    max_records: 1,
                    max_payload_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES as u64,
                    max_record_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES as u64,
                },
                records: vec![RecordWireRecord {
                    position: u64::MAX.to_string(),
                    id: escaped,
                    payload: STANDARD.encode(vec![0xff; MAX_PHYSICAL_RECORD_PAYLOAD_BYTES]),
                }],
            },
        )
        .unwrap();
        assert!(record.len() > MAX_PAYLOAD_BYTES as usize);
        assert!(record.len() <= MAX_RECORD_RESPONSE_BYTES);

        let (release, gate) = tokio::sync::oneshot::channel();
        let (socket, mut peer) = test_socket(Some(gate));
        let (sink, _incoming) = socket.split();
        let (control_send, controls) = mpsc::channel(4);
        let (refusal_send, refusals) = mpsc::channel(1);
        let (ordinary_send, ordinary) = mpsc::channel(16);
        let (record_send, records) = mpsc::channel(1);
        let slots = Arc::new(Semaphore::new(1));
        let control_slots = Arc::new(Semaphore::new(1));
        let record_capacity = Arc::new(Semaphore::new(1));
        let subscriptions = Arc::new(SubscriptionDeliveries::new(Default::default()));
        let writer = tokio::spawn(write_authenticated(
            sink,
            controls,
            no_close(),
            refusals,
            ordinary,
            records,
            Duration::from_secs(1),
            Arc::default(),
            subscriptions.clone(),
        ));
        record_send
            .send(QueuedRecordResponse::new(QueuedResponse {
                message: WireResponse::record(record.clone()),
                _slot: slots.clone().try_acquire_owned().unwrap().into(),
                _record_work: Some(RecordReadLease::new(
                    record_capacity.clone().try_acquire_owned().unwrap(),
                )),
                _mount: None,
            }))
            .await
            .unwrap();
        timeout(Duration::from_secs(1), peer.writing.recv())
            .await
            .expect("maximum record starts its physical send")
            .unwrap();
        control_send
            .send(ControlOutput::Response(Box::new(QueuedResponse {
                message: WireResponse::ordinary(success("control", &json!({}))),
                _slot: control_slots.clone().try_acquire_owned().unwrap().into(),
                _record_work: None,
                _mount: None,
            })))
            .await
            .unwrap();
        // The record's send is stalled: its capacity came back when the writer
        // took it (row R64); the control still holds its own.
        assert_eq!(slots.available_permits(), 1);
        assert_eq!(record_capacity.available_permits(), 1);
        // One extra answer can be retained while the first send is active.
        // It owns the only record slot, so a third read cannot be admitted.
        record_send
            .send(QueuedRecordResponse::new(QueuedResponse {
                message: WireResponse::record("{}".into()),
                _slot: slots.clone().try_acquire_owned().unwrap().into(),
                _record_work: Some(RecordReadLease::new(
                    record_capacity.clone().try_acquire_owned().unwrap(),
                )),
                _mount: None,
            }))
            .await
            .unwrap();
        assert_eq!(slots.available_permits(), 0);
        assert_eq!(record_capacity.available_permits(), 0);
        assert!(slots.clone().try_acquire_owned().is_err());
        release.send(()).unwrap();
        let Message::Text(first) = peer.message().await else {
            panic!("record response expected")
        };
        assert_eq!(first.as_str(), record);
        let Message::Text(second) = peer.message().await else {
            panic!("control response expected")
        };
        let value: Value = serde_json::from_str(&second).unwrap();
        assert_eq!(value["id"], "control");
        let Message::Text(third) = peer.message().await else {
            panic!("second record response expected")
        };
        assert_eq!(third.as_str(), "{}");
        // Close the same delivery interest as the production connection owner.
        subscriptions.close();
        drop(control_send);
        drop(refusal_send);
        drop(ordinary_send);
        drop(record_send);
        writer.await.unwrap();
        assert_eq!(slots.available_permits(), 1);
        assert_eq!(control_slots.available_permits(), 1);
        assert_eq!(record_capacity.available_permits(), 1);

        let (mut socket, _peer) = test_socket(None);
        let oversized_ordinary = success("ordinary", &json!({"body": "x".repeat(record.len())}));
        assert!(
            send(Duration::from_secs(1), &mut socket, oversized_ordinary)
                .await
                .is_err()
        );
    }

    /// Row R64: a real server WebSocket over an in-memory duplex, as the
    /// product's sink, noting the socket's free record slot and global read
    /// permits at the first call that can write a byte.
    struct ObservedWebSocket {
        inner: WebSocketStream<DuplexStream>,
        slots: Arc<Semaphore>,
        reads: Arc<Semaphore>,
        free_at_first_write: Option<(usize, usize)>,
    }
    impl ObservedWebSocket {
        fn observe(&mut self) {
            let free = (
                self.slots.available_permits(),
                self.reads.available_permits(),
            );
            self.free_at_first_write.get_or_insert(free);
        }
    }
    impl Sink<Message> for ObservedWebSocket {
        type Error = Error;
        fn poll_ready(
            self: Pin<&mut Self>,
            context: &mut std::task::Context<'_>,
        ) -> Poll<Result<(), Error>> {
            let this = self.get_mut();
            this.observe();
            Pin::new(&mut this.inner)
                .poll_ready(context)
                .map_err(Error::new)
        }
        fn start_send(self: Pin<&mut Self>, message: Message) -> Result<(), Error> {
            let this = self.get_mut();
            this.observe();
            let Message::Text(text) = message else {
                panic!("the record lane sends text")
            };
            let text = Frame::text(text.as_str());
            Pin::new(&mut this.inner)
                .start_send(text)
                .map_err(Error::new)
        }
        fn poll_flush(
            self: Pin<&mut Self>,
            context: &mut std::task::Context<'_>,
        ) -> Poll<Result<(), Error>> {
            let this = self.get_mut();
            this.observe();
            Pin::new(&mut this.inner)
                .poll_flush(context)
                .map_err(Error::new)
        }
        fn poll_close(
            self: Pin<&mut Self>,
            context: &mut std::task::Context<'_>,
        ) -> Poll<Result<(), Error>> {
            Pin::new(&mut self.get_mut().inner)
                .poll_close(context)
                .map_err(Error::new)
        }
    }

    /// The client pings before each read, so tungstenite has a pong due and
    /// writes it with the answer inside `start_send`. The record slot and read
    /// permit are free before any byte of either answer can reach the client,
    /// for a record and for a refusal on the record lane.
    #[tokio::test]
    async fn record_capacity_is_free_before_any_byte_reaches_a_pinging_client() {
        let (server, client) = tokio::io::duplex(4 * MAX_RECORD_RESPONSE_BYTES);
        let mut client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
        let slots = Arc::new(Semaphore::new(1));
        let reads = Arc::new(Semaphore::new(4));
        let mut socket = ObservedWebSocket {
            inner: WebSocketStream::from_raw_socket(server, Role::Server, None).await,
            slots: slots.clone(),
            reads: reads.clone(),
            free_at_first_write: None,
        };
        for message in [
            WireResponse::record("{}".into()),
            WireResponse::record("x".repeat(MAX_RECORD_RESPONSE_BYTES)),
            WireResponse::ordinary(failure("read", "source_preparing")),
        ] {
            client.send(Frame::Ping(vec![1].into())).await.unwrap();
            // The server reads the ping, which queues its pong.
            let ping = socket.inner.next().await.unwrap().unwrap();
            assert!(matches!(ping, Frame::Ping(_)), "{ping:?}");
            let slot = Arc::new(slots.clone().try_acquire_owned().unwrap());
            let permit = reads.clone().try_acquire_owned().unwrap();
            let response = QueuedRecordResponse::owned(
                message,
                Some(slot.clone()),
                Some(RecordReadLease::new((permit, slot))),
                None,
            );
            socket.free_at_first_write = None;
            assert!(
                send_record_queued(Duration::from_secs(1), &mut socket, response)
                    .await
                    .is_ok()
            );
            assert_eq!(socket.free_at_first_write, Some((1, 4)));
            // The pong and the answer both arrive, in tungstenite's order.
            let mut kinds = Vec::new();
            for _ in 0..2 {
                kinds.push(
                    match timeout(Duration::from_secs(1), client.next())
                        .await
                        .expect("pong and answer arrive")
                        .unwrap()
                        .unwrap()
                    {
                        Frame::Pong(_) => "pong",
                        Frame::Text(_) => "text",
                        other => panic!("unexpected frame {}", other.len()),
                    },
                );
            }
            kinds.sort_unstable();
            assert_eq!(kinds, ["pong", "text"]);
        }
    }

    #[tokio::test]
    async fn record_send_uses_its_absolute_deadline_and_encoded_ceiling() {
        let (captured, _guard) = limit_log();
        let (mut socket, mut peer) = test_socket(None);
        assert!(send_queued(
            Duration::from_secs(1),
            &mut socket,
            WireResponse::record("x".repeat(MAX_RECORD_RESPONSE_BYTES + 1)),
        )
        .await
        .is_err());
        assert!(peer.writing.try_recv().is_err());
        assert!(
            limit_text(&captured).contains("product.max_record_response_bytes"),
            "{}",
            limit_text(&captured)
        );

        let (_release, gate) = tokio::sync::oneshot::channel();
        let (mut stalled, mut stalled_peer) = test_socket(Some(gate));
        let expired = QueuedRecordResponse {
            message: WireResponse::record("{}".into()),
            _slot: Some(
                Arc::new(Semaphore::new(1))
                    .try_acquire_owned()
                    .unwrap()
                    .into(),
            ),
            _record_work: None,
            _mount: None,
            deadline: Instant::now() - Duration::from_millis(1),
        };
        assert!(timeout(
            Duration::from_secs(1),
            send_record_queued(Duration::from_secs(30), &mut stalled, expired)
        )
        .await
        .expect("expired response settles")
        .is_err());
        assert!(stalled_peer.writing.try_recv().is_err());
        assert!(stalled_peer.output.try_recv().is_err());
        assert!(
            limit_text(&captured).contains("socket.record_delivery_deadline"),
            "{}",
            limit_text(&captured)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn ready_record_send_refuses_expired_frames_before_sink_effects() {
        for refusal in [false, true] {
            for elapsed in [29, 30, 31] {
                let (mut socket, mut peer) = test_socket(None);
                let slots = Arc::new(Semaphore::new(1));
                let reads = Arc::new(Semaphore::new(1));
                let response = QueuedRecordResponse::new(QueuedResponse {
                    message: if refusal {
                        WireResponse::ordinary(failure("refusal", "read_timeout"))
                    } else {
                        WireResponse::record("{\"record\":true}".into())
                    },
                    _slot: slots.clone().try_acquire_owned().unwrap().into(),
                    _record_work: (!refusal)
                        .then(|| RecordReadLease::new(reads.clone().try_acquire_owned().unwrap())),
                    _mount: None,
                });
                let sending = send_record_queued(Duration::from_secs(5), &mut socket, response);
                tokio::time::advance(Duration::from_secs(elapsed)).await;
                let result = sending.await;
                assert_eq!(
                    result.is_ok(),
                    elapsed < 30,
                    "refusal {refusal}, elapsed {elapsed}"
                );
                assert_eq!(peer.writing.try_recv().is_ok(), elapsed < 30);
                assert_eq!(peer.output.try_recv().is_ok(), elapsed < 30);
                assert_eq!(slots.available_permits(), 1);
                assert_eq!(reads.available_permits(), 1);
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn timed_out_record_frame_ends_writer_before_any_later_control_frame() {
        let (_release, gate) = tokio::sync::oneshot::channel();
        let (socket, mut peer) = test_socket(Some(gate));
        let (sink, _incoming) = socket.split();
        let (control_send, controls) = mpsc::channel(4);
        let (_refusal_send, refusals) = mpsc::channel(1);
        let (_ordinary_send, ordinary) = mpsc::channel(16);
        let (record_send, records) = mpsc::channel(1);
        let slots = Arc::new(Semaphore::new(2));
        let writer = tokio::spawn(write_authenticated(
            sink,
            controls,
            no_close(),
            refusals,
            ordinary,
            records,
            Duration::from_secs(5),
            Arc::default(),
            Arc::new(SubscriptionDeliveries::new(Default::default())),
        ));
        record_send
            .send(QueuedRecordResponse::new(QueuedResponse {
                message: WireResponse::record("{\"type\":\"res\"}".into()),
                _slot: slots.clone().try_acquire_owned().unwrap().into(),
                _record_work: None,
                _mount: None,
            }))
            .await
            .unwrap();
        peer.writing.recv().await.unwrap();
        control_send
            .send(ControlOutput::Response(Box::new(QueuedResponse {
                message: WireResponse::ordinary(success("control", &json!({}))),
                _slot: slots.clone().try_acquire_owned().unwrap().into(),
                _record_work: None,
                _mount: None,
            })))
            .await
            .unwrap();
        // Only the queued control's slot is held while the record's send is
        // stalled (row R64).
        assert_eq!(slots.available_permits(), 1);
        tokio::time::advance(RECORD_SEND_TIMEOUT + Duration::from_millis(1)).await;
        timeout(Duration::from_secs(1), writer)
            .await
            .expect("timed-out writer stops instead of sending queued control")
            .unwrap();
        assert!(peer.output.try_recv().is_err());
        assert_eq!(slots.available_permits(), 2);
    }

    #[tokio::test]
    async fn a_failed_writer_ends_receive_ownership_without_another_request() {
        let (state, _) = fixture(MembershipRole::Member);
        let session = authenticate(&state).await;
        let (socket, peer) = test_socket(None);
        peer.request("health");
        let TestPeer {
            input: _still_connected,
            output,
            writing: _writing,
        } = peer;
        drop(output);
        let owner = tokio::spawn(run_authenticated(socket, state, session));
        timeout(Duration::from_secs(1), owner)
            .await
            .expect("writer failure ends the authenticated socket owner")
            .unwrap();
    }
    #[tokio::test(start_paused = true)]
    async fn crossing_a_wall_second_does_not_reject_an_in_window_authentication() {
        struct MillisecondClock(AtomicU64);
        impl Clock for MillisecondClock {
            fn unix_milliseconds(&self) -> u64 {
                self.0.load(Ordering::SeqCst)
            }
        }
        let (mut state, _) = fixture(MembershipRole::Member);
        let clock = Arc::new(MillisecondClock(AtomicU64::new(100_999)));
        state.clock = clock.clone();
        state.settings = state
            .settings
            .with_deadlines(Some(Duration::from_secs(1)), None);
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(handle_socket(socket, state));
        let Message::Text(challenge) = peer.message().await else {
            panic!("challenge expected")
        };
        let challenge: Value = serde_json::from_str(&challenge).unwrap();
        assert_eq!(challenge["payload"]["expiresAt"], 102);
        tokio::time::advance(Duration::from_millis(500)).await;
        // The old independent seconds check rejected this while its timer still ran.
        clock.0.store(101_499, Ordering::SeqCst);
        peer.input
            .send(Ok(Message::Text(
                json!({
                    "type": "req", "id": "auth", "method": "session.authenticate", "params": {
                        "minVersion": 1, "maxVersion": 1, "nonce": challenge["payload"]["nonce"],
                        "credential": "secret", "client": {"id": "test"},
                        "surface": {"kind": "cli", "instance": "test"}
                    }
                })
                .to_string()
                .into(),
            )))
            .unwrap();
        assert_success(peer.message().await, "auth");
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn repeated_credential_key_never_reaches_authentication() {
        // Two credentials on the wire are not one agreed credential. Frame
        // decoding rejects the frame while both are still visible, so nothing
        // downstream has to guess which one the sender meant.
        let (mut state, _) = fixture(MembershipRole::Member);
        state.settings = state
            .settings
            .with_deadlines(Some(Duration::from_secs(1)), None);
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(handle_socket(socket, state));
        let Message::Text(challenge) = peer.message().await else {
            panic!("challenge expected")
        };
        let challenge: Value = serde_json::from_str(&challenge).unwrap();
        let nonce = challenge["payload"]["nonce"].as_str().unwrap();
        peer.input
            .send(Ok(Message::Text(
                format!(
                    r#"{{"type":"req","id":"auth","method":"session.authenticate","params":{{"minVersion":1,"maxVersion":1,"nonce":"{nonce}","credential":"wrong","credential":"secret","client":{{"id":"test"}},"surface":{{"kind":"cli","instance":"test"}}}}}}"#
                )
                .into(),
            )))
            .unwrap();
        let Message::Text(text) = peer.message().await else {
            panic!("unauthorized expected")
        };
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["id"], "");
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["code"], "unauthorized");
        let Message::Close(Some(close)) = peer.message().await else {
            panic!("close expected")
        };
        assert_eq!(close.code, 4001);
        task.await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handshake_records_each_surface_and_keeps_the_credential_principal() {
        let (captured, _guard) = info_log();
        for (kind, instance) in [("desktop", "window-a"), ("panel", "window-b")] {
            let (state, _) = fixture(MembershipRole::Member);
            let (socket, mut peer) = test_socket(None);
            let task = tokio::spawn(handle_socket(socket, state));
            let Message::Text(challenge) = peer.message().await else {
                panic!("challenge expected")
            };
            let challenge: Value = serde_json::from_str(&challenge).unwrap();
            peer.input
                .send(Ok(Message::Text(
                    json!({
                        "type": "req", "id": "auth", "method": "session.authenticate",
                        "params": {
                            "minVersion": 1, "maxVersion": 1,
                            "nonce": challenge["payload"]["nonce"],
                            "credential": "secret",
                            "client": {"id": "nessa-panel"},
                            "surface": {"kind": kind, "instance": instance}
                        }
                    })
                    .to_string()
                    .into(),
                )))
                .unwrap();
            let Message::Text(text) = peer.message().await else {
                panic!("ready expected")
            };
            let value: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value["ok"], true, "{value}");
            assert_eq!(value["payload"]["principalId"], "principal");
            task.abort();
        }
        let text = limit_text(&captured);
        assert!(text.contains("surface_kind=\"desktop\""), "{text}");
        assert!(text.contains("surface_instance=\"window-a\""), "{text}");
        assert!(text.contains("surface_kind=\"panel\""), "{text}");
        assert!(text.contains("surface_instance=\"window-b\""), "{text}");
        assert!(text.contains("principal_id=\"principal\""), "{text}");
        assert!(text.contains("client_id=\"nessa-panel\""), "{text}");
        assert!(!text.contains("credential="), "{text}");
        assert!(!text.contains("\"secret\""), "{text}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handshake_refuses_a_surface_the_schema_does_not_admit() {
        let (captured, _guard) = info_log();
        let (state, _) = fixture(MembershipRole::Member);
        let defects = [
            json!({}),
            json!({"kind": "phone", "instance": "window"}),
            json!({"kind": "desktop", "instance": ""}),
            json!({"kind": "desktop", "instance": "window", "role": "admin"}),
        ];
        for surface in defects {
            let (socket, mut peer) = test_socket(None);
            let task = tokio::spawn(handle_socket(socket, state.clone()));
            let Message::Text(challenge) = peer.message().await else {
                panic!("challenge expected")
            };
            let challenge: Value = serde_json::from_str(&challenge).unwrap();
            let mut params = json!({
                "minVersion": 1, "maxVersion": 1,
                "nonce": challenge["payload"]["nonce"],
                "credential": "secret",
                "client": {"id": "test"},
                "surface": {"kind": "desktop", "instance": "window"}
            });
            if surface.as_object().unwrap().is_empty() {
                params.as_object_mut().unwrap().remove("surface");
            } else {
                params["surface"] = surface;
            }
            peer.input
                .send(Ok(Message::Text(
                    json!({
                        "type": "req", "id": "auth", "method": "session.authenticate",
                        "params": params
                    })
                    .to_string()
                    .into(),
                )))
                .unwrap();
            let Message::Text(text) = peer.message().await else {
                panic!("refusal expected")
            };
            let value: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value["id"], "auth");
            assert_eq!(value["ok"], false);
            assert_eq!(value["error"]["code"], "unauthorized");
            let Message::Close(Some(close)) = peer.message().await else {
                panic!("close expected")
            };
            assert_eq!(close.code, 4001);
            task.abort();
        }
        assert!(
            !limit_text(&captured).contains("authenticated product session"),
            "{}",
            limit_text(&captured)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn surface_instance_bound_counts_characters_and_names_the_limit() {
        let (captured, _guard) = info_log();
        let exact = "😀".repeat(MAX_PRODUCT_SURFACE_INSTANCE_CHARACTERS);
        let over = "😀".repeat(MAX_PRODUCT_SURFACE_INSTANCE_CHARACTERS + 1);
        for (instance, admitted) in [(exact, true), (over, false)] {
            let (state, _) = fixture(MembershipRole::Member);
            let (socket, mut peer) = test_socket(None);
            let task = tokio::spawn(handle_socket(socket, state));
            let Message::Text(challenge) = peer.message().await else {
                panic!("challenge expected")
            };
            let challenge: Value = serde_json::from_str(&challenge).unwrap();
            peer.input
                .send(Ok(Message::Text(
                    json!({
                        "type": "req", "id": "auth", "method": "session.authenticate",
                        "params": {
                            "minVersion": 1, "maxVersion": 1,
                            "nonce": challenge["payload"]["nonce"],
                            "credential": "secret",
                            "client": {"id": "test"},
                            "surface": {"kind": "web", "instance": instance}
                        }
                    })
                    .to_string()
                    .into(),
                )))
                .unwrap();
            let Message::Text(text) = peer.message().await else {
                panic!("response expected")
            };
            let value: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value["ok"], admitted, "{value}");
            if !admitted {
                assert_eq!(value["error"]["code"], "unauthorized");
            }
            task.abort();
        }
        let text = limit_text(&captured);
        assert!(
            text.contains("product.max_surface_instance_characters"),
            "{text}"
        );
        assert!(text.contains("surface_kind=\"web\""), "{text}");
        assert!(!text.contains("\"secret\""), "{text}");
    }

    #[tokio::test]
    async fn repeated_nested_grant_key_is_rejected_before_administration() {
        // Collapsed, this frame asks for a supported grant and reaches the
        // credential store; the unsupported action it also carried would never
        // be seen again. An administration that answers "unavailable" for every
        // call it receives is how this test tells the two apart.
        let (state, _) = fixture(MembershipRole::Admin);
        let state = state.with_admin(Arc::new(RejectingAdmin(CredentialAdminError::Unavailable)));
        let session = authenticate(&state).await;
        for grant in [
            r#"{"action":"admin.write","action":"server.read","resource":{"organizationId":"organization","id":"gateway-resource"}}"#,
            r#"{"action":"server.read","action":"admin.write","resource":{"organizationId":"organization","id":"gateway-resource"}}"#,
            r#"{"action":"server.read","action":"server.read","resource":{"organizationId":"organization","id":"gateway-resource"}}"#,
        ] {
            let (socket, mut peer) = test_socket(None);
            let task = tokio::spawn(run_authenticated(socket, state.clone(), session.clone()));
            peer.input
                .send(Ok(Message::Text(
                    format!(
                        r#"{{"type":"req","id":"issue","method":"credential.issue","params":{{"requestId":"issue","principal":{{"id":"reader","kind":"integration"}},"membership":{{"id":"reader","principalId":"reader","organizationId":"organization","role":"member","state":"active"}},"grants":[{grant}]}}}}"#
                    )
                    .into(),
                )))
                .unwrap();
            let Message::Text(text) = peer.message().await else {
                panic!("response expected")
            };
            let value: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value["id"], "issue");
            assert_eq!(value["ok"], false);
            assert_eq!(value["error"]["code"], "invalid_request");
            task.abort();
        }
    }

    #[tokio::test]
    async fn a_frame_naming_its_request_twice_is_answered_to_neither() {
        // A nested duplicate still has one `id`, so it gets a correlated error.
        // A duplicated `id` does not, and the server will not choose one.
        let (state, _) = fixture(MembershipRole::Member);
        let session = authenticate(&state).await;
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, state, session));
        peer.input
            .send(Ok(Message::Text(
                r#"{"type":"req","id":"first","id":"second","method":"server.health","params":{}}"#
                    .into(),
            )))
            .unwrap();
        // The next frame is well formed, so its reply is the one that arrives.
        peer.request("after");
        let Message::Text(text) = peer.message().await else {
            panic!("response expected")
        };
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["id"], "after");
        task.abort();
    }

    #[tokio::test]
    async fn an_ordinary_issue_frame_still_reaches_administration() {
        let (state, _) = fixture(MembershipRole::Admin);
        let state = state.with_admin(Arc::new(RejectingAdmin(CredentialAdminError::Unavailable)));
        let session = authenticate(&state).await;
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, state, session));
        peer.input
            .send(Ok(Message::Text(
                json!({"type":"req","id":"issue","method":"credential.issue","params":issue_params()})
                    .to_string()
                    .into(),
            )))
            .unwrap();
        let Message::Text(text) = peer.message().await else {
            panic!("response expected")
        };
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["error"]["code"], "credential_store_unavailable");
        task.abort();
    }

    /// The registry, not the socket, decides what a credential may carry and
    /// who it may be issued to; its refusal reaches the wire as
    /// `credential_conflict`. Against a real store, a grant this principal's
    /// kind may not hold and an admin membership are both refused that way,
    /// and the same frame asking for an ordinary grant is issued.
    #[tokio::test]
    async fn the_store_refuses_an_unsupported_issue_as_a_credential_conflict() {
        use nessa_auth::adapters::local::{BootstrapRequest, LocalCredentialStore};
        use nessa_auth::application::dto::{
            MembershipInputDto, MembershipRoleDto, OrganizationInputDto, PrincipalInputDto,
            PrincipalKindDto,
        };
        let directory = tempfile::tempdir().unwrap();
        let auth = directory.path().join("auth");
        nessa_local_storage::create_directory(&auth).unwrap();
        let store = LocalCredentialStore::open(
            &auth,
            "credentials.json",
            Arc::new(CedarPolicyEvaluator::new().unwrap()),
        )
        .unwrap();
        store
            .bootstrap(BootstrapRequest {
                gateway_id: "gateway".into(),
                organization: OrganizationInputDto {
                    id: "organization".into(),
                },
                principal: PrincipalInputDto {
                    id: "principal".into(),
                    kind: PrincipalKindDto::Human,
                },
                membership: MembershipInputDto {
                    id: "membership".into(),
                    principal_id: "principal".into(),
                    organization_id: "organization".into(),
                    role: MembershipRoleDto::Admin,
                    state: MembershipStateDto::Active,
                },
                credential_id: "credential".into(),
                issued_at: 50,
                expires_at: None,
                grants: vec![CredentialGrantDto {
                    action: "credential.manage".into(),
                    resource: ResourceDto {
                        organization_id: "organization".into(),
                        id: "gateway".into(),
                    },
                }],
            })
            .unwrap();
        let (state, _) = fixture(MembershipRole::Admin);
        let state = state.with_admin(Arc::new(store));
        let session = authenticate(&state).await;
        let issue = |request: &str, action: &str, role: &str| {
            let mut params = issue_params();
            params["requestId"] = json!(request);
            params["membership"]["role"] = json!(role);
            params["grants"][0]["action"] = json!(action);
            params["grants"][0]["resource"]["id"] = json!("gateway");
            json!({"type":"req","id":request,"method":"credential.issue","params":params})
        };
        for (frame, expected) in [
            (
                issue("serve", "environment.serve", "member"),
                Err("credential_conflict"),
            ),
            (
                issue("admin", "server.read", "admin"),
                Err("credential_conflict"),
            ),
            (issue("read", "server.read", "member"), Ok(())),
        ] {
            let (socket, mut peer) = test_socket(None);
            let task = tokio::spawn(run_authenticated(socket, state.clone(), session.clone()));
            peer.input
                .send(Ok(Message::Text(frame.to_string().into())))
                .unwrap();
            let Message::Text(text) = peer.message().await else {
                panic!("response expected")
            };
            let value: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value["id"], frame["id"], "{value}");
            match expected {
                Err(code) => assert_eq!(value["error"]["code"], code, "{value}"),
                Ok(()) => assert_eq!(value["ok"], true, "{value}"),
            }
            task.abort();
        }
    }

    #[tokio::test(start_paused = true)]
    async fn elapsed_handshake_closes_with_retryable_timeout() {
        let (mut state, _) = fixture(MembershipRole::Member);
        state.settings = state
            .settings
            .with_deadlines(Some(Duration::from_secs(1)), None);
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(handle_socket(socket, state));
        assert!(matches!(peer.message().await, Message::Text(_)));
        tokio::time::advance(Duration::from_millis(1001)).await;
        let Message::Close(Some(close)) = peer.message().await else {
            panic!("close expected")
        };
        assert_eq!(close.code, 4006);
        let reason: Value = serde_json::from_str(&close.reason).unwrap();
        assert_eq!(reason["code"], "handshake_timeout");
        assert_eq!(reason["retryable"], true);
        task.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn challenge_write_consumes_the_same_handshake_budget() {
        let (mut state, _) = fixture(MembershipRole::Member);
        state.settings = state
            .settings
            .with_deadlines(Some(Duration::from_secs(1)), Some(Duration::from_secs(5)));
        let (release, gate) = tokio::sync::oneshot::channel();
        let (socket, mut peer) = test_socket(Some(gate));
        let task = tokio::spawn(handle_socket(socket, state));
        peer.writing.recv().await.unwrap();
        tokio::time::advance(Duration::from_millis(1001)).await;
        tokio::task::yield_now().await;
        release.send(()).unwrap();
        // The cancelled write may have queued the challenge; it must not start a new window.
        let mut message = peer.message().await;
        if matches!(message, Message::Text(_)) {
            message = peer.message().await;
        }
        let Message::Close(Some(close)) = message else {
            panic!("close expected")
        };
        assert_eq!(close.code, 4006);
        task.await.unwrap();
    }

    fn assert_success(message: Message, id: &str) {
        let Message::Text(text) = message else {
            panic!("expected successful response")
        };
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["id"], id);
        assert_eq!(value["ok"], true);
    }
    fn revoke(authority: &Authority) {
        let mut snapshot = authority.snapshot.lock().unwrap();
        snapshot.credential.restore_revoked_at(100).unwrap();
        snapshot.revision += 1;
    }

    #[tokio::test]
    async fn slow_socket_does_not_block_other_sessions_or_revocation() {
        let (state, authority) = fixture(MembershipRole::Member);
        let session = authenticate(&state).await;
        let (release, gate) = tokio::sync::oneshot::channel();
        let (slow_socket, mut slow) = test_socket(Some(gate));
        let (fast_socket, mut fast) = test_socket(None);
        let slow_task = tokio::spawn(run_authenticated(
            slow_socket,
            state.clone(),
            session.clone(),
        ));
        let fast_task = tokio::spawn(run_authenticated(fast_socket, state, session));
        slow.request("slow");
        timeout(Duration::from_secs(1), slow.writing.recv())
            .await
            .unwrap()
            .unwrap();
        // This must complete while the first socket's flush is still blocked.
        fast.request("fast");
        assert_success(fast.message().await, "fast");
        revoke(&authority);
        fast.request("after-revoke");
        let Message::Close(Some(close)) = fast.message().await else {
            panic!("revoked session must close")
        };
        assert_eq!(
            close.code,
            SessionCloseReason::CredentialRevoked.web_socket_code()
        );
        // Already admitted output is allowed to finish even after publication.
        release.send(()).unwrap();
        assert_success(slow.message().await, "slow");
        drop(slow.input);
        drop(fast.input);
        timeout(Duration::from_secs(1), slow_task)
            .await
            .unwrap()
            .unwrap();
        timeout(Duration::from_secs(1), fast_task)
            .await
            .unwrap()
            .unwrap();
    }

    struct RevokeAfterAdmission {
        authority: Arc<Authority>,
        policy: CedarPolicyEvaluator,
    }

    struct PresenceStore {
        present: Arc<AtomicBool>,
        session: BrowserSessionState,
    }
    impl SessionStore for PresenceStore {
        fn insert<'a>(
            &'a self,
            _: String,
            _: BrowserSessionState,
            _: Option<String>,
            _: u64,
        ) -> PortFuture<'a, Option<(String, BrowserSessionState)>> {
            Box::pin(async { Err(AccessError::Unsupported) })
        }
        fn get<'a>(&'a self, _: String) -> PortFuture<'a, Option<BrowserSessionState>> {
            Box::pin(async move {
                Ok(self
                    .present
                    .load(Ordering::SeqCst)
                    .then(|| self.session.clone()))
            })
        }
        fn remove<'a>(
            &'a self,
            _: String,
            _: u64,
            _: RemovalReason,
            _: Option<CredentialId>,
        ) -> PortFuture<'a, ()> {
            Box::pin(async move {
                self.present.store(false, Ordering::SeqCst);
                Ok(())
            })
        }
        fn abandon_login<'a>(
            &'a self,
            _: String,
            _: Option<(String, BrowserSessionState)>,
            _: u64,
        ) -> PortFuture<'a, ()> {
            Box::pin(async { Err(AccessError::Unsupported) })
        }
        fn renew<'a>(
            &'a self,
            _: String,
            _: u64,
            _: CredentialId,
        ) -> PortFuture<'a, BrowserSessionState> {
            Box::pin(async { Err(AccessError::Unsupported) })
        }
    }

    struct FailingPresenceStore {
        inner: PresenceStore,
        reads: AtomicU64,
        fail_at: u64,
        error: AccessError,
    }
    impl SessionStore for FailingPresenceStore {
        fn insert<'a>(
            &'a self,
            id: String,
            session: BrowserSessionState,
            prior: Option<String>,
            now: u64,
        ) -> PortFuture<'a, Option<(String, BrowserSessionState)>> {
            self.inner.insert(id, session, prior, now)
        }
        fn get<'a>(&'a self, id: String) -> PortFuture<'a, Option<BrowserSessionState>> {
            if self.reads.fetch_add(1, Ordering::SeqCst) + 1 == self.fail_at {
                Box::pin(async { Err(self.error) })
            } else {
                self.inner.get(id)
            }
        }
        fn remove<'a>(
            &'a self,
            id: String,
            now: u64,
            reason: RemovalReason,
            actor: Option<CredentialId>,
        ) -> PortFuture<'a, ()> {
            self.inner.remove(id, now, reason, actor)
        }
        fn abandon_login<'a>(
            &'a self,
            id: String,
            prior: Option<(String, BrowserSessionState)>,
            now: u64,
        ) -> PortFuture<'a, ()> {
            self.inner.abandon_login(id, prior, now)
        }
        fn renew<'a>(
            &'a self,
            id: String,
            now: u64,
            credential: CredentialId,
        ) -> PortFuture<'a, BrowserSessionState> {
            self.inner.renew(id, now, credential)
        }
    }

    #[tokio::test]
    async fn passive_browser_presence_preserves_retryable_store_failures() {
        for error in [
            AccessError::Unavailable,
            AccessError::StaleRevision,
            AccessError::Unsupported,
        ] {
            // Current identity reads twice; the third read rechecks presence.
            for fail_at in [3, 1] {
                for method in [
                    "conversation.recordsHead",
                    "conversation.recordsPage",
                    "conversation.catalogueHead",
                    "conversation.catalogueManifest",
                    "conversation.catalogueResolve",
                ] {
                    let (mut state, _) = fixture(MembershipRole::Member);
                    let session = authenticate(&state).await;
                    let present = Arc::new(AtomicBool::new(true));
                    let origin = "https://127.0.0.1:1443";
                    let store = Arc::new(FailingPresenceStore {
                        inner: PresenceStore {
                            present: present.clone(),
                            session: BrowserSessionState::new(
                                session.context().credential_id().clone(),
                                BrowserSessionOrigin::new(origin.to_owned()).unwrap(),
                                100,
                            )
                            .unwrap(),
                        },
                        reads: AtomicU64::new(0),
                        fail_at,
                        error,
                    });
                    state.browser_sessions = Some(store.clone());
                    state.browser_session_id = Some("a".repeat(64));
                    state.browser_session_origin = Some(origin.to_owned());
                    state.record_source = Some(Arc::new(UnreachableRecordSource));
                    let permits = Arc::new(Semaphore::new(1));
                    let (response, lease) = dispatch_passive_read(
                        &state,
                        &session,
                        request("browser-outage", method),
                        RecordReadLease::new(permits.clone().try_acquire_owned().unwrap()),
                        Instant::now() + PASSIVE_READ_TIMEOUT,
                    )
                    .await;
                    let WireResponse::Ordinary(message) = response else {
                        panic!("ordinary refusal expected")
                    };
                    let OutgoingMessage::Response(response) = *message else {
                        panic!("correlated refusal expected")
                    };
                    assert_eq!(response.id, "browser-outage");
                    assert_eq!(
                        response.error.unwrap().code,
                        "unverifiable",
                        "{error:?}, browser read {fail_at}, {method}"
                    );
                    assert_eq!(store.reads.load(Ordering::SeqCst), fail_at);
                    assert!(present.load(Ordering::SeqCst));
                    assert!(lease.is_none());
                    assert_eq!(permits.available_permits(), 1);
                }
            }
        }
    }

    struct RemoveBrowserAfterAdmission {
        present: Arc<AtomicBool>,
        policy: CedarPolicyEvaluator,
    }

    struct RemoveBrowserAfterAuthorityRead {
        authority: Arc<Authority>,
        present: Arc<AtomicBool>,
    }
    impl AccessReader for RemoveBrowserAfterAuthorityRead {
        fn read<'a>(&'a self, id: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
            Box::pin(async move {
                let snapshot = self.authority.read(id).await?;
                self.present.store(false, Ordering::SeqCst);
                Ok(snapshot)
            })
        }
    }
    impl PolicyEvaluator for RemoveBrowserAfterAdmission {
        fn evaluate(
            &self,
            context: &AuthContext,
            action: &Action,
            resource: &Resource,
            snapshot: &AccessSnapshot,
        ) -> Result<Decision, AccessError> {
            let decision = self.policy.evaluate(context, action, resource, snapshot)?;
            if decision == Decision::Allow {
                self.present.store(false, Ordering::SeqCst);
            }
            Ok(decision)
        }
    }

    #[tokio::test]
    async fn browser_logout_after_policy_allow_prevents_handler_dispatch() {
        let (mut state, _) = fixture(MembershipRole::Member);
        let session = authenticate(&state).await;
        let present = Arc::new(AtomicBool::new(true));
        let store = Arc::new(PresenceStore {
            present: present.clone(),
            session: BrowserSessionState::new(
                session.context().credential_id().clone(),
                BrowserSessionOrigin::new("https://127.0.0.1:1443".to_owned()).unwrap(),
                100,
            )
            .unwrap(),
        });
        state = state.with_browser_sessions(store);
        state.browser_session_id = Some("a".repeat(64));
        state.browser_session_origin = Some("https://127.0.0.1:1443".into());
        state.policy = Arc::new(RemoveBrowserAfterAdmission {
            present,
            policy: CedarPolicyEvaluator::new().unwrap(),
        });
        state.uptime_clock = Arc::new(MustNotRun);

        let OutgoingMessage::Response(response) =
            dispatch(&state, &session, request("logout-race", "server.health")).await
        else {
            panic!("response expected")
        };
        assert_eq!(response.error.unwrap().code, "unauthorized");
    }

    #[tokio::test]
    async fn browser_logout_after_auth_snapshot_prevents_auth_session_success() {
        let (mut state, authority) = fixture(MembershipRole::Member);
        let session = authenticate(&state).await;
        let present = Arc::new(AtomicBool::new(true));
        let store = Arc::new(PresenceStore {
            present: present.clone(),
            session: BrowserSessionState::new(
                session.context().credential_id().clone(),
                BrowserSessionOrigin::new("https://127.0.0.1:1443".to_owned()).unwrap(),
                100,
            )
            .unwrap(),
        });
        state = state.with_browser_sessions(store);
        state.browser_session_id = Some("a".repeat(64));
        state.browser_session_origin = Some("https://127.0.0.1:1443".into());
        state.access = Arc::new(RemoveBrowserAfterAuthorityRead { authority, present });

        let OutgoingMessage::Response(response) =
            dispatch(&state, &session, request("logout-race", "auth.session")).await
        else {
            panic!("response expected")
        };
        assert_eq!(response.error.unwrap().code, "unauthorized");
    }
    impl PolicyEvaluator for RevokeAfterAdmission {
        fn evaluate(
            &self,
            context: &AuthContext,
            action: &Action,
            resource: &Resource,
            snapshot: &AccessSnapshot,
        ) -> Result<Decision, AccessError> {
            let decision = self.policy.evaluate(context, action, resource, snapshot)?;
            if decision == Decision::Allow {
                revoke(&self.authority);
            }
            Ok(decision)
        }
    }
    #[tokio::test]
    async fn revocation_after_admission_does_not_discard_the_operation_response() {
        let (mut state, authority) = fixture(MembershipRole::Member);
        let session = authenticate(&state).await;
        state.policy = Arc::new(RevokeAfterAdmission {
            authority,
            policy: CedarPolicyEvaluator::new().unwrap(),
        });
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, state, session));
        peer.request("admitted");
        assert_success(peer.message().await, "admitted");
        peer.request("later");
        let Message::Close(Some(close)) = peer.message().await else {
            panic!("next operation must see revocation")
        };
        assert_eq!(
            close.code,
            SessionCloseReason::CredentialRevoked.web_socket_code()
        );
        timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
    }
    #[derive(Clone)]
    struct LimitLog(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for LimitLog {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("limit log").extend_from_slice(buffer);
            Ok(buffer.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LimitLog {
        type Writer = Self;
        fn make_writer(&'a self) -> Self {
            self.clone()
        }
    }
    fn info_log() -> (LimitLog, tracing::subscriber::DefaultGuard) {
        let captured = LimitLog(Arc::new(std::sync::Mutex::new(Vec::new())));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(captured.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .finish();
        (captured, tracing::subscriber::set_default(subscriber))
    }
    fn limit_log() -> (LimitLog, tracing::subscriber::DefaultGuard) {
        let captured = LimitLog(Arc::new(std::sync::Mutex::new(Vec::new())));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(captured.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .finish();
        (captured, tracing::subscriber::set_default(subscriber))
    }
    fn limit_text(captured: &LimitLog) -> String {
        String::from_utf8(captured.0.lock().expect("limit log").clone()).unwrap()
    }
    fn named(captured: &LimitLog, limit: &str) {
        let text = limit_text(captured);
        assert!(text.contains(limit), "{text}");
    }

    #[tokio::test]
    async fn a_closed_server_limit_is_named_on_the_refusal() {
        for (method, limit, assign) in [
            ("server.health", "server.requests", AssignLimit::Requests),
            (
                "conversation.cancel",
                "server.controls",
                AssignLimit::Controls,
            ),
            (
                "conversation.recordsHead",
                "server.record_reads",
                AssignLimit::RecordReads,
            ),
            (
                "conversation.delete",
                "server.deletions",
                AssignLimit::Deletions,
            ),
            (
                "attachment.begin",
                "server.upload_begins",
                AssignLimit::Uploads,
            ),
            (
                "artifact.read",
                "server.artifact_reads",
                AssignLimit::ArtifactReads,
            ),
        ] {
            let (captured, _guard) = limit_log();
            let (mut state, _) = fixture(MembershipRole::Member);
            let closed = Arc::new(Semaphore::new(0));
            match assign {
                AssignLimit::Requests => state.requests = closed,
                AssignLimit::Controls => state.controls = closed,
                AssignLimit::RecordReads => state.record_reads = closed,
                AssignLimit::Deletions => state.deletions = closed,
                AssignLimit::Uploads => state.upload_begins = closed,
                AssignLimit::ArtifactReads => state.artifact_reads = closed,
            }
            let session = authenticate(&state).await;
            let (socket, mut peer) = test_socket(None);
            let task = tokio::spawn(run_authenticated(socket, state, session));
            peer.input
                .send(Ok(Message::Text(
                    json!({"type": "req", "id": "busy", "method": method, "params": {}})
                        .to_string()
                        .into(),
                )))
                .unwrap();
            let Message::Text(text) = peer.message().await else {
                panic!("refusal expected for {method}")
            };
            let value: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value["id"], "busy");
            assert_eq!(value["error"]["code"], "temporarily_unavailable");
            named(&captured, limit);
            task.abort();
        }
    }

    async fn admit(peer: &TestPeer, method: &str, id: &str) {
        peer.input
            .send(Ok(Message::Text(
                json!({"type": "req", "id": id, "method": method, "params": {}})
                    .to_string()
                    .into(),
            )))
            .unwrap();
        for _ in 0..24 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn a_full_socket_slot_set_names_that_limit() {
        for (method, held, limit) in [
            ("server.health", 16, "socket.ordinary_slots"),
            ("conversation.cancel", 4, "socket.control_slots"),
            ("conversation.recordsHead", 1, "socket.record_slot"),
            ("mcp.callTool", 4, "socket.app_calls"),
        ] {
            let (captured, _guard) = limit_log();
            let (state, _) = fixture(MembershipRole::Member);
            let session = authenticate(&state).await;
            let (_release, gate) = tokio::sync::oneshot::channel();
            let (socket, mut peer) = test_socket(Some(gate));
            let task = tokio::spawn(run_authenticated(socket, state, session));
            // A record reply gives its slot back before the physical send, so the
            // writer has to be parked on an earlier frame or that slot is free
            // again by the time the next record is admitted.
            let stall = if method == "conversation.recordsHead" {
                "server.health"
            } else {
                method
            };
            admit(&peer, stall, "stall").await;
            timeout(Duration::from_secs(1), peer.writing.recv())
                .await
                .unwrap_or_else(|_| panic!("{method} did not reach the stalled writer"));
            let already = usize::from(stall == method);
            for id in already..held {
                admit(&peer, method, &format!("held-{id}")).await;
            }
            admit(&peer, method, "overflow").await;
            let text = limit_text(&captured);
            assert!(text.contains(limit), "{method} log:\n{text}");
            task.abort();
        }
    }

    /// A client asking for more than its slots and the refusal lane hold
    /// while the writer is stalled keeps its socket: reading waits for the
    /// lane, and every frame is answered.
    #[tokio::test]
    async fn a_client_past_its_slots_is_refused_frame_by_frame_not_cut_off() {
        let (state, _) = fixture(MembershipRole::Member);
        let session = authenticate(&state).await;
        let (release, gate) = tokio::sync::oneshot::channel();
        let (socket, mut peer) = test_socket(Some(gate));
        let task = tokio::spawn(run_authenticated(socket, state, session));
        admit(&peer, "server.health", "held-0").await;
        timeout(Duration::from_secs(1), peer.writing.recv())
            .await
            .expect("the stalled writer has taken the first response");
        for id in 1..16 {
            admit(&peer, "server.health", &format!("held-{id}")).await;
        }
        for id in 0..4 {
            admit(&peer, "server.health", &format!("over-{id}")).await;
        }
        release.send(()).unwrap();
        let mut refused = Vec::new();
        let mut answered = 0;
        while answered < 20 {
            let Message::Text(text) = peer.message().await else {
                continue;
            };
            let value: Value = serde_json::from_str(&text).unwrap();
            answered += 1;
            if value["error"]["code"] == "temporarily_unavailable" {
                refused.push(value["id"].as_str().unwrap().to_owned());
            }
        }
        // The first two past the slots are refused; later ones may find a
        // slot the writer gave back by then.
        assert!(
            refused.starts_with(&["over-0".to_owned(), "over-1".to_owned()]),
            "{refused:?}"
        );
        assert!(
            refused.iter().all(|id| id.starts_with("over-")),
            "{refused:?}"
        );
        assert!(!task.is_finished(), "the socket stays open");
        task.abort();
    }

    /// A refusal waiting for the lane does not stop the socket's loop: the
    /// gateway closing the socket is acted on while the writer is stalled, so
    /// once the writer is free the close follows the frame it was sending.
    #[tokio::test]
    async fn a_waiting_refusal_does_not_stop_the_socket_closing() {
        let (state, _) = fixture(MembershipRole::Member);
        let session = authenticate(&state).await;
        let (release, gate) = tokio::sync::oneshot::channel();
        let (socket, mut peer) = test_socket(Some(gate));
        let task = tokio::spawn(run_authenticated(socket, state.clone(), session));
        admit(&peer, "server.health", "held-0").await;
        timeout(Duration::from_secs(1), peer.writing.recv())
            .await
            .expect("the stalled writer has taken the first response");
        for id in 1..16 {
            admit(&peer, "server.health", &format!("held-{id}")).await;
        }
        // The first refusal takes the lane's place; the second waits.
        admit(&peer, "server.health", "over-0").await;
        admit(&peer, "server.health", "over-1").await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        state.close_sockets();
        tokio::time::sleep(Duration::from_millis(50)).await;
        release.send(()).unwrap();
        let mut sent = Vec::new();
        let close = loop {
            match peer.message().await {
                Message::Close(close) => break close,
                Message::Text(text) => {
                    let value: Value = serde_json::from_str(&text).unwrap();
                    sent.push(value["id"].as_str().unwrap_or_default().to_owned());
                }
                _ => {}
            }
        };
        assert_eq!(
            close.map(|close| close.code),
            Some(SessionCloseReason::TemporaryUnavailable.web_socket_code())
        );
        // Stuck waiting for the lane, the loop would see the close only once
        // the writer had sent the queued refusal and every held reply.
        assert_eq!(sent, ["held-0"], "the close follows the stalled frame");
        timeout(Duration::from_secs(5), task)
            .await
            .expect("the socket ends once closed")
            .unwrap();
    }

    #[tokio::test]
    async fn a_full_refusal_lane_names_that_limit() {
        let (captured, _guard) = limit_log();
        let (state, _) = fixture(MembershipRole::Member);
        let session = authenticate(&state).await;
        let (_release, gate) = tokio::sync::oneshot::channel();
        let (socket, mut peer) = test_socket(Some(gate));
        let task = tokio::spawn(run_authenticated(socket, state, session));
        admit(&peer, "server.health", "held-0").await;
        timeout(Duration::from_secs(1), peer.writing.recv())
            .await
            .expect("the stalled writer has taken the first response");
        for id in 1..16 {
            admit(&peer, "server.health", &format!("held-{id}")).await;
        }
        admit(&peer, "server.health", "overflow").await;
        admit(&peer, "server.health", "dropped").await;
        named(&captured, "socket.ordinary_slots");
        named(&captured, "socket.refusal_lane");
        task.abort();
    }

    #[tokio::test]
    async fn an_oversized_frame_names_the_payload_limit() {
        let (captured, _guard) = limit_log();
        let (state, _) = fixture(MembershipRole::Member);
        let session = authenticate(&state).await;
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, state, session));
        peer.input
            .send(Ok(Message::Text(
                "x".repeat(MAX_PAYLOAD_BYTES as usize + 1).into(),
            )))
            .unwrap();
        timeout(Duration::from_secs(1), task)
            .await
            .expect("an oversized frame ends the session")
            .unwrap();
        named(&captured, "product.max_payload_bytes");
        assert!(peer.output.try_recv().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn a_stalled_ordinary_record_reply_names_the_write_timeout() {
        let (captured, _guard) = limit_log();
        let (_release, gate) = tokio::sync::oneshot::channel();
        let (mut socket, _peer) = test_socket(Some(gate));
        let response = QueuedRecordResponse {
            message: WireResponse::ordinary(success("read", &json!({}))),
            _slot: Some(
                Arc::new(Semaphore::new(1))
                    .try_acquire_owned()
                    .unwrap()
                    .into(),
            ),
            _record_work: None,
            _mount: None,
            deadline: Instant::now() + Duration::from_secs(30),
        };
        let task = tokio::spawn(async move {
            send_record_queued(Duration::from_secs(5), &mut socket, response).await
        });
        tokio::time::advance(Duration::from_secs(5)).await;
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        assert!(task
            .await
            .expect("the stalled record reply finishes")
            .is_err());
        let logged = limit_text(&captured);
        assert!(logged.contains("socket.write_timeout"), "{logged}");
        assert!(
            !logged.contains("socket.record_delivery_deadline"),
            "{logged}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_stalled_write_names_the_write_timeout() {
        let (captured, _guard) = limit_log();
        let (_release, gate) = tokio::sync::oneshot::channel();
        let (mut socket, _peer) = test_socket(Some(gate));
        let task = tokio::spawn(async move {
            send(
                Duration::from_secs(5),
                &mut socket,
                success("id", &json!({})),
            )
            .await
        });
        tokio::time::advance(Duration::from_secs(5)).await;
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        let result = task.await.expect("the stalled write finishes");
        assert!(result.is_err());
        named(&captured, "socket.write_timeout");
    }

    #[tokio::test(start_paused = true)]
    async fn a_stalled_close_names_the_write_timeout() {
        let (captured, _guard) = limit_log();
        let (_release, gate) = tokio::sync::oneshot::channel();
        let (mut socket, _peer) = test_socket(Some(gate));
        let task = tokio::spawn(async move {
            close_session(
                Duration::from_secs(5),
                &mut socket,
                SessionCloseReason::CredentialExpired,
            )
            .await
        });
        tokio::time::advance(Duration::from_secs(5)).await;
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        task.await.expect("the stalled close finishes");
        named(&captured, "socket.write_timeout");
    }

    enum AssignLimit {
        Requests,
        Controls,
        RecordReads,
        Deletions,
        Uploads,
        ArtifactReads,
    }

    include!("../../tests/conversation/gateway.rs");
    mod catalogue_receiver {
        include!("../../tests/conversation/catalogue_receiver/product.rs");
    }
    mod record_receiver {
        include!("../../tests/conversation/record_receiver.rs");
    }
    include!("../../tests/attachments/gateway.rs");
    include!("../../tests/agent_install/gateway.rs");
    include!("../../tests/mcp_servers/gateway.rs");
}
