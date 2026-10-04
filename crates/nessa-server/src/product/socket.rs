use super::change_watch::{
    ConnectionWatches, WatchAcknowledgement, WatchDeliveries, WatchFrame, WatchOutcome, WatchReply,
};
use super::passive_read::deadlines::{PASSIVE_READ_TIMEOUT, RECORD_SEND_TIMEOUT};
use super::record_read::refusal_code;
use super::state::ProductRouteState;
use super::wire::ready_frame;
use crate::browser_session::application::{
    invalidation_reason, BrowserSessionVerifier, ReadBrowserSession,
};
use crate::browser_session::domain::value_objects::RemovalReason;
use crate::conversation::application::{access_refusal, RecordReadLease};
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
use nessa_auth::application::dto::{
    CredentialGrantDto, MembershipRoleDto, MembershipStateDto, ResourceDto,
};
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
    CredentialIssueParams, CredentialListParams, CredentialListResult, CredentialRevokeParams,
    CredentialRevokeResult, ExistingCredentialResult, IssuedCredentialResult, ProductSessionReady,
    SessionAuthenticateParams, SessionChallenge, SessionTermination, MAX_RECORD_RESPONSE_BYTES,
    PRODUCT_HANDSHAKE_METHOD, PRODUCT_READY_METHODS, PRODUCT_VERSION,
};
use nessa_protocol::product::handshake::{authentication_close_reason, supports_product_version};
use nessa_protocol::product_contract::generated::SessionCloseReason;
use nessa_protocol::protocol::{
    health_check_message, unique_envelope, EventFrame, OutgoingMessage, RequestFrame,
    ResponseFrame, MAX_PAYLOAD_BYTES,
};
use serde_json::json;
use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;
use tokio::sync::mpsc::Receiver;
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};
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
    let (request_id, session) = match authenticated {
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

async fn receive_authentication<S, P>(
    socket: &mut S,
    state: &ProductRouteState,
    nonce: &str,
    deadline: Instant,
    proof: &P,
) -> Result<(String, AuthenticatedSession), (String, &'static str)>
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
    if params.nonce != nonce || params.client.id.is_empty() || params.client.id.len() > 256 {
        return Err((frame.id, "unauthorized"));
    }
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
        return Ok((frame.id, identity));
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
    Ok((frame.id, session))
}

fn credential_admin_code(error: CredentialAdminError) -> &'static str {
    match error {
        CredentialAdminError::Conflict => "credential_conflict",
        CredentialAdminError::Capacity => "credential_capacity",
        CredentialAdminError::NotFound => "credential_not_found",
        CredentialAdminError::Unavailable => "credential_store_unavailable",
    }
}

/// How many MCP App calls one socket has running at once; past that each is
/// refused `temporarily_unavailable` (`protocol/README.md`).
const APP_CALLS_PER_SOCKET: usize = 4;

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
            | "conversation.archive"
            | "conversation.unarchive"
            | "conversation.answer"
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
            "mcp.callTool" | "mcp.readResource" => Self::App,
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
}

enum WireResponse {
    Ordinary(Box<OutgoingMessage>),
    Record { text: String },
    Watch(Box<QueuedWatch>),
}

struct QueuedWatch {
    message: Box<OutgoingMessage>,
    acknowledgement: WatchAcknowledgement,
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
    deadline: Instant,
}
impl QueuedRecordResponse {
    fn new(response: QueuedResponse) -> Self {
        Self::owned(
            response.message,
            Some(response._slot),
            response._record_work,
        )
    }

    // A refused admission owns delivery only, never a fabricated read permit.
    fn refusal(message: OutgoingMessage) -> Self {
        Self::owned(WireResponse::ordinary(message), None, None)
    }

    fn owned(
        message: WireResponse,
        slot: Option<Arc<OwnedSemaphorePermit>>,
        record_work: Option<RecordReadLease>,
    ) -> Self {
        Self {
            message,
            _slot: slot,
            _record_work: record_work,
            deadline: Instant::now() + RECORD_SEND_TIMEOUT,
        }
    }
}

enum ControlOutput {
    Response(Box<QueuedResponse>),
    Close(SessionCloseReason),
}

enum WriterResponse {
    Record(Box<QueuedRecordResponse>),
    Queued(Box<QueuedResponse>),
    Refusal(Box<OutgoingMessage>),
    Watch(Box<WatchFrame>),
}

async fn write_authenticated<S>(
    mut sink: SplitSink<S, Message>,
    mut controls: Receiver<ControlOutput>,
    mut refusals: Receiver<OutgoingMessage>,
    mut ordinary: Receiver<QueuedResponse>,
    mut records: Receiver<QueuedRecordResponse>,
    write_timeout: Duration,
    watches: Arc<WatchDeliveries>,
) where
    S: Stream<Item = Result<Message, Error>> + Sink<Message> + Unpin,
{
    // Moving the record out of its lane observes its original deadline, not a
    // physical-send priority change. The retained-position bound is documented in
    // docs/design/authorized-record-reads.md, R62.
    let mut pending_record: Option<QueuedRecordResponse> = None;
    loop {
        let deadline = retained_delivery_deadline(&pending_record, &watches);
        let next = tokio::select! {
            biased;
            () = wait_for_record_deadline(deadline), if deadline.is_some() => break,
            Some(response) = records.recv(), if pending_record.is_none() => {
                pending_record = Some(response);
                continue;
            },
            Some(control) = controls.recv() => match control {
                ControlOutput::Response(response) => Some(Ok(WriterResponse::Queued(response))),
                ControlOutput::Close(reason) => Some(Err(reason)),
            },
            Some(response) = refusals.recv() => Some(Ok(WriterResponse::Refusal(Box::new(response)))),
            Some(response) = ordinary.recv() => Some(Ok(WriterResponse::Queued(Box::new(response)))),
            () = std::future::ready(()), if pending_record.is_some() =>
                Some(Ok(WriterResponse::Record(Box::new(pending_record.take().expect("pending record selected"))))),
            // A notice leaves WatchDeliveries only when this arm is chosen, so an
            // unwatch that retires it before then also removes it (row U3).
            Some(frame) = async { watches.take() }, if !watches.is_closed() =>
                Some(Ok(WriterResponse::Watch(Box::new(frame)))),
            () = watches.changed(), if !watches.is_closed() => continue,
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
                    } = *response;
                    send_queued(write_timeout, &mut sink, message).await.is_ok()
                }
                Some(Ok(WriterResponse::Watch(frame))) => {
                    let WatchFrame {
                        id,
                        message,
                        deadline,
                        terminal,
                        _owner: _original_owner,
                    } = *frame;
                    let result =
                        within_deadline(deadline, send(write_timeout, &mut sink, message)).await;
                    if matches!(result, Some(Ok(()))) {
                        watches.sent(&id, terminal);
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
            let deadline = retained_delivery_deadline(&pending_record, &watches);
            tokio::select! {
                biased;
                // Abandon this sink; a second frame must not follow a cancelled
                // physical write. Also observe a record arriving during it.
                () = wait_for_record_deadline(deadline), if deadline.is_some() => return,
                Some(response) = records.recv(), if pending_record.is_none() =>
                    pending_record = Some(response),
                () = watches.changed(), if !watches.is_closed() => {},
                succeeded = &mut writing => {
                    if !succeeded { return; }
                    break;
                },
            }
        }
    }
}

/// The socket's event sequence: `session.challenge` is its first event, and
/// every event after authentication (watch notices, from `WatchDeliveries`)
/// continues from here, so one socket never repeats a sequence number.
pub(super) const CHALLENGE_EVENT_SEQUENCE: u64 = 1;

fn retained_delivery_deadline(
    record: &Option<QueuedRecordResponse>,
    watches: &WatchDeliveries,
) -> Option<Instant> {
    [queued_record_deadline(record), watches.deadline()]
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

enum AuthenticatedInput {
    Request(RequestFrame),
    Refusal(OutgoingMessage),
}

type AuthorityCheck<'a> = Pin<Box<dyn Future<Output = Option<AccessError>> + Send + 'a>>;
type RefreshCheck<'a> =
    Pin<Box<dyn Future<Output = Result<AuthenticatedSession, AccessError>> + Send + 'a>>;

async fn run_authenticated<S>(socket: S, state: ProductRouteState, session: AuthenticatedSession)
where
    S: Stream<Item = Result<Message, Error>> + Sink<Message> + Unpin + Send + 'static,
{
    let (sink, mut incoming) = socket.split();
    let mut watches = ConnectionWatches::new(&state);
    let (control_send, control_receive) = mpsc::channel(4);
    let (refusal_send, refusal_receive) = mpsc::channel(1);
    // Room for every ordinary slot's response and every app call's, which
    // share it: a full queue closes the socket.
    let (ordinary_send, ordinary_receive) = mpsc::channel(16 + APP_CALLS_PER_SOCKET);
    let (record_send, record_receive) = mpsc::channel(1);
    let mut writer = tokio::spawn(write_authenticated(
        sink,
        control_receive,
        refusal_receive,
        ordinary_receive,
        record_receive,
        state.settings.write_timeout(),
        watches.deliveries.clone(),
    ));
    let control_slots = Arc::new(Semaphore::new(4));
    let ordinary_slots = Arc::new(Semaphore::new(16));
    let record_slots = Arc::new(Semaphore::new(1));
    let app_slots = Arc::new(Semaphore::new(APP_CALLS_PER_SOCKET));
    // An app call still running when the socket goes is cancelled: its
    // review, if it is waiting on one, is withdrawn rather than left standing
    // for nobody. One already sent finishes, and is recorded, on its own task.
    let mut app_calls: Vec<tokio::task::AbortHandle> = Vec::new();
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
        watches.collect_retired();
        // Authority checks suspend only their own admission. Request completion,
        // delivery teardown and expiry retain independently polled owners (R60).
        let admitted = tokio::select! {
            _ = &mut writer => {
                writer_finished = true;
                break;
            }
            _ = state.change_watches.closed() => {
                let _ = control_send.try_send(ControlOutput::Close(SessionCloseReason::TemporaryUnavailable));
                break;
            }
            outcome = watches.next(&state, &session), if watches.has_pending() => {
                match outcome {
                    WatchOutcome::Reply(reply) => { if ordinary_send.try_send(queued_watch(*reply)).is_err() { break; } }
                    WatchOutcome::Close(reason) => { let _ = control_send.try_send(ControlOutput::Close(reason)); break; }
                    WatchOutcome::Progress => {},
                }
                None
            }
            Some(result) = requests.next(), if !requests.is_empty() => {
                let Ok((message, class, slot, record_work)) = result else { break };
                let queued = QueuedResponse { message, _slot: slot, _record_work: record_work };
                let sent = match class {
                    ResponseClass::Control => control_send.try_send(ControlOutput::Response(Box::new(queued))).is_ok(),
                    ResponseClass::Ordinary | ResponseClass::App => ordinary_send.try_send(queued).is_ok(),
                    ResponseClass::Record => record_send.try_send(QueuedRecordResponse::new(queued)).is_ok(),
                };
                if !sent { break; }
                None
            }
            _ = &mut expiry => {
                let _ = control_send.try_send(ControlOutput::Close(SessionCloseReason::CredentialExpired));
                break;
            }
            current = async { refresh.as_mut().expect("refresh selected while pending").await }, if refresh.is_some() => {
                refresh = None;
                match current {
                    // The one identity check per tick also confirms each live
                    // watch's session; the watches then re-ask only their
                    // passive-read admission (row A3).
                    Ok(current) => watches.recheck(&state, &current),
                    Err(error) => {
                        let _ = control_send.try_send(ControlOutput::Close(close_reason(error)));
                        break;
                    }
                }
                None
            }
            _ = current_state.tick(), if refresh.is_none() => {
                refresh = Some(Box::pin(current_session(&state, &session)));
                None
            }
            error = async { input_check.as_mut().expect("input check selected while pending").await }, if input_check.is_some() => {
                input_check = None;
                if let Some(error) = error {
                    let _ = control_send.try_send(ControlOutput::Close(close_reason(error)));
                    break;
                }
                match pending_input.take().expect("input check owns one input") {
                    AuthenticatedInput::Request(frame) => Some((frame, Instant::now())),
                    AuthenticatedInput::Refusal(response) => {
                        if refusal_send.try_send(response).is_err() { break; }
                        None
                    }
                }
            }
            message = incoming.next(), if pending_input.is_none() => {
                let Some(Ok(message)) = message else { break };
                let Message::Text(text) = message else {
                    if matches!(message, Message::Close(_)) { break; }
                    continue;
                };
                if text.len() > MAX_PAYLOAD_BYTES as usize { break; }
                let frame: RequestFrame = match RequestFrame::decode(&text) {
                    Ok(frame) => frame,
                    Err(_) => {
                        let Some(response) = correlatable_invalid_request(&text) else { continue };
                        pending_input = Some(AuthenticatedInput::Refusal(response));
                        input_check = Some(Box::pin(current_session_error(&state, &session)));
                        continue;
                    },
                };
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
        };
        let Some((frame, received_at)) = admitted else {
            continue;
        };
        let read_deadline = received_at + PASSIVE_READ_TIMEOUT;
        let class = ResponseClass::for_method(&frame.method);
        let control = matches!(class, ResponseClass::Control);
        let record = matches!(class, ResponseClass::Record);
        let app = matches!(class, ResponseClass::App);
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
            let response = failure(&frame.id, "temporarily_unavailable");
            let refused = if record {
                record_send
                    .try_send(QueuedRecordResponse::refusal(response))
                    .is_err()
            } else {
                refusal_send.try_send(response).is_err()
            };
            if refused {
                break;
            }
            continue;
        };
        let slot = Arc::new(slot);
        if ConnectionWatches::method(&frame.method) {
            if let Some(reply) = watches.begin(
                &state,
                &session,
                frame,
                slot,
                received_at + RECORD_SEND_TIMEOUT,
            ) {
                if ordinary_send.try_send(queued_watch(reply)).is_err() {
                    break;
                }
            }
            continue;
        }
        // An app call's capacity across sockets is the conversation
        // service's, held by the call's own task until it ends: a permit held
        // here would be let go when the socket went, while the call ran on.
        let capacity = if control {
            Some(&state.controls)
        } else if record {
            Some(&state.record_reads)
        } else if app {
            None
        } else if frame.method == "conversation.delete" {
            Some(&state.deletions)
        } else if frame.method == "attachment.begin" {
            Some(&state.upload_begins)
        } else {
            Some(&state.requests)
        };
        let permit = capacity.map(|capacity| capacity.clone().try_acquire_owned());
        let Ok(permit) = permit.transpose() else {
            let response = failure(&frame.id, "temporarily_unavailable");
            let queued = QueuedResponse {
                message: WireResponse::ordinary(response),
                _slot: slot,
                _record_work: None,
            };
            let sent = if control {
                control_send
                    .try_send(ControlOutput::Response(Box::new(queued)))
                    .is_ok()
            } else if record {
                record_send
                    .try_send(QueuedRecordResponse::new(queued))
                    .is_ok()
            } else {
                ordinary_send.try_send(queued).is_ok()
            };
            if !sent {
                break;
            }
            continue;
        };
        let request_state = state.clone();
        let request_session = session.clone();
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
                (message, class, slot, record_work)
            } else {
                let _permit = permit;
                let message = dispatch(&request_state, &request_session, frame).await;
                (WireResponse::ordinary(message), class, slot, None)
            }
        });
        if app {
            app_calls.retain(|call| !call.is_finished());
            app_calls.push(task.abort_handle());
        }
        requests.push(task);
    }
    for call in app_calls {
        call.abort();
    }
    drop(watches);
    drop(control_send);
    drop(refusal_send);
    drop(ordinary_send);
    drop(record_send);
    if !writer_finished {
        let _ = writer.await;
    }
}

fn queued_watch(reply: WatchReply) -> QueuedResponse {
    let message = match reply.acknowledgement {
        Some(acknowledgement) => WireResponse::Watch(Box::new(QueuedWatch {
            message: Box::new(reply.message),
            acknowledgement,
        })),
        None => WireResponse::ordinary(reply.message),
    };
    QueuedResponse {
        message,
        _slot: reply.slot,
        _record_work: None,
    }
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

async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
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
    let action_name = match action_for_method(&frame.method) {
        Some(action) => action,
        _ => return failure(&frame.id, "unknown_method"),
    };
    let authorization = authorize(state, &session, action_name).await;
    match authorization {
        Ok(Decision::Allow) => {
            if ensure_browser_session_present(state, &session)
                .await
                .is_err()
            {
                failure(&frame.id, "unauthorized")
            } else {
                dispatch_authorized(state, &session, frame).await
            }
        }
        Ok(Decision::Deny) => failure(&frame.id, "forbidden"),
        Err(_) => failure(&frame.id, "unauthorized"),
    }
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

async fn dispatch_authorized(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    match frame.method.as_str() {
        "agents.list" => {
            if frame.params != json!({}) {
                return failure(&frame.id, "invalid_request");
            }
            match state.agents_catalog.as_ref() {
                Some(catalog) => success(&frame.id, catalog.as_ref()),
                None => failure(&frame.id, "conversations_not_configured"),
            }
        }
        "agents.installOptions" | "agents.install" => {
            super::agent_install::dispatch(state, session, frame).await
        }
        method if method.starts_with("conversation.") => {
            super::conversation::dispatch(state, session, frame).await
        }
        method if method.starts_with("attachment.") => {
            super::attachment::dispatch(state, session, frame).await
        }
        method if method.starts_with("mcp.") => {
            super::mcp_apps::dispatch(state, session, frame).await
        }
        method if method.starts_with("pairing.") => {
            super::pairing::dispatch(state, session, frame).await
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
            let grants_supported = params.grants.iter().all(|grant| {
                matches!(
                    grant.action.as_str(),
                    "server.read" | "conversation.read" | "conversation.write"
                ) && grant.resource.organization_id == context.organization_id().as_str()
                    && grant.resource.id == state.gateway_id().as_str()
            });
            if params.request_id.is_empty()
                || params.request_id.len() > 200
                || params.membership.organization_id != context.organization_id().as_str()
                || params.membership.principal_id != params.principal.id
                || !matches!(params.membership.role, MembershipRoleDto::Member)
                || !matches!(params.membership.state, MembershipStateDto::Active)
                || params
                    .expires_at
                    .is_some_and(|expiry| expiry <= now || expiry > 9_007_199_254_740_991)
                || params.grants.is_empty()
                || !grants_supported
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

fn action_for_method(method: &str) -> Option<&'static str> {
    match method {
        "server.health" | "agents.list" | "agents.installOptions" => Some("server.read"),
        "conversation.recordsHead" | "conversation.recordsPage" | "conversation.catalogueHead" | "conversation.catalogueManifest" | "conversation.catalogueResolve" => Some("conversation.read"),
        "agents.install"
        | "conversation.create"
        | "conversation.setApprovalMode"
        | "conversation.read"
        | "conversation.list"
        | "conversation.send"
        | "conversation.steer"
        | "conversation.remove"
        | "conversation.reorder"
        | "conversation.answer"
        // Answering the agent's own question is input to the conversation, so
        // it is writing to it like any other reply.
        | "conversation.answerQuestion"
        | "conversation.cancel"
        | "conversation.close"
        | "conversation.archive"
        | "conversation.unarchive"
        | "conversation.delete"
        // Uploading into a conversation is writing to it.
        | "attachment.begin"
        // An app acts in its conversation, on its caller's behalf.
        | "mcp.callTool"
        | "mcp.readResource"
        | "mcp.releaseApp" => Some("conversation.write"),
        "credential.issue" | "credential.list" | "credential.revoke" => Some("credential.manage"),
        // Enrolling a device creates a credential for it; Auth asks again for
        // the exact consent inside the runtime.
        "pairing.create"
        | "pairing.pending"
        | "pairing.status"
        | "pairing.approve"
        | "pairing.deny"
        | "pairing.cancel" => Some("credential.manage"),
        _ => None,
    }
}

// Which request to blame for a frame that did not decode. A nested duplicate
// still leaves one unambiguous `id`, and so does a nested string that is not
// Unicode: that frame is answered `invalid_request`. A frame that named `id`
// twice, or whose `id` is not itself a string, has no single request to answer,
// and the server does not pick one. That frame gets no reply, as any other
// uncorrelatable text does, and the client's own request timeout settles it.
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

// Watches retain this whole adapter await in their original owner task. No
// timeout here may detach an internal authority worker from that owner.
pub(super) async fn watch_identity(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
) -> Result<AuthenticatedSession, AccessError> {
    current_identity_inner(state, session)
        .await
        .map(|(current, _)| current)
}

pub(super) async fn watch_browser_present(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
) -> Result<(), AccessError> {
    ensure_browser_session_present(state, session).await
}

/// The connection's current session, or why it is no longer current.
async fn current_session(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
) -> Result<AuthenticatedSession, AccessError> {
    current_identity(state, session)
        .await
        .map(|(current, _)| current)
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
    timeout(write_timeout, send_text(socket, text))
        .await
        .map_err(|_| ())?
}

/// An ordinary message's wire text, refused past the ordinary encoded ceiling.
fn ordinary_text(message: OutgoingMessage) -> Result<String, ()> {
    let text = message.to_wire_text().map_err(|_| ())?;
    if text.len() > MAX_PAYLOAD_BYTES as usize {
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
        WireResponse::Watch(response) => {
            let QueuedWatch {
                message,
                acknowledgement,
            } = *response;
            let WatchAcknowledgement {
                deadline,
                completed,
                owner: _original_owner,
            } = acknowledgement;
            // A reply the writer reaches after its deadline is replaced by a
            // typed close, never a silent end (row B6). Expiry during the
            // write abandons the socket like any cancelled write.
            if Instant::now() >= deadline {
                close_session(
                    write_timeout,
                    socket,
                    SessionCloseReason::TemporaryUnavailable,
                )
                .await;
                return Err(());
            }
            within_deadline(deadline, send(write_timeout, socket, *message))
                .await
                .ok_or(())??;
            let _ = completed.send(());
            Ok(())
        }
        WireResponse::Record { text } => {
            if text.len() > MAX_RECORD_RESPONSE_BYTES {
                return Err(());
            }
            socket
                .send(Message::Text(text.into()))
                .await
                .map_err(|_| ())
        }
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
            drop((slot, record_work));
            return within_deadline(deadline, timeout(write_timeout, send_text(socket, text)))
                .await
                .ok_or(())?
                .map_err(|_| ())?;
        }
        WireResponse::Record { text } => text,
        // The record lane carries the five passive methods' answers only:
        // watch replies go through `ConnectionWatches` on the ordinary lane, so
        // this arm is never reached. It is still delivered as `send_queued`
        // would, under this response's deadline, rather than dropped.
        message @ WireResponse::Watch(_) => {
            drop((slot, record_work));
            return within_deadline(deadline, send_queued(write_timeout, socket, message))
                .await
                .ok_or(())?;
        }
    };
    if text.len() > MAX_RECORD_RESPONSE_BYTES {
        return Err(());
    }
    drop((slot, record_work));
    within_deadline(deadline, send_text(socket, text))
        .await
        .ok_or(())?
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
    let _ = timeout(write_timeout, socket.send(close)).await;
}

#[cfg(test)]
pub(crate) use tests::watches::HostWatchFixture;

#[cfg(test)]
mod tests {
    use super::super::state::SessionSettings;
    use super::*;
    use crate::agents_test_support::StubAgentProbe;
    use crate::browser_session::application::SessionStore;
    use crate::browser_session::domain::value_objects::{
        BrowserSessionOrigin, BrowserSessionState,
    };
    use crate::conversation::application::{
        ConversationRepository, ReceiverAuthority, ReceiverBinding, RecordReadError,
        RecordReadFuture, RecordReadOperation, RecordReadResponse, RecordReadSource,
    };
    use crate::conversation::domain::Conversation;
    use crate::conversation::infrastructure::{LocalConversationStore, NessaRecordReadSource};
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
    use nessa_sdk::application::agent_execution::providers::ProviderIdentity;
    use nessa_sdk::application::agent_execution::sessions::{
        ProviderContext, SessionChange, SessionSaveUnit, SessionSnapshot, SessionStorage,
    };
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
    use tokio_tungstenite::tungstenite::{protocol::Role, Message as Frame};
    use tokio_tungstenite::WebSocketStream;
    use uuid::Uuid;

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
    pub(super) mod watches {
        include!("../../tests/product/socket/watches.rs");
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
            if *method == "auth.session" || ConnectionWatches::method(method) {
                continue;
            }
            assert!(
                action_for_method(method).is_some(),
                "advertised method has no authorization/dispatch path: {method}"
            );
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
        fn read<'a>(&'a self, _: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
            Box::pin(async move {
                Ok(self
                    .snapshot
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone())
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
        let organization = OrganizationId::new("organization").unwrap();
        AccessSnapshot {
            credential: Credential::new(
                CredentialId::new("credential").unwrap(),
                PrincipalId::new("principal").unwrap(),
                organization.clone(),
                AudienceId::new("gateway").unwrap(),
                100,
                200,
                vec![
                    Grant::new(
                        Action::new("server.read").unwrap(),
                        Resource::new(
                            organization.clone(),
                            ResourceId::new("gateway-resource").unwrap(),
                        ),
                    ),
                    Grant::new(
                        Action::new("credential.manage").unwrap(),
                        Resource::new(
                            organization.clone(),
                            ResourceId::new("gateway-resource").unwrap(),
                        ),
                    ),
                ],
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

    fn fixture(role: MembershipRole) -> (ProductRouteState, Arc<Authority>) {
        let authority = Arc::new(Authority {
            snapshot: Mutex::new(snapshot(role, MembershipStatus::Active)),
            now: AtomicU64::new(100),
            proof_expires_at: Mutex::new(Some(200)),
        });
        let state = ProductRouteState::new(
            ResourceId::new("gateway-resource").unwrap(),
            OrganizationId::new("organization").unwrap(),
            AudienceId::new("gateway").unwrap(),
            ProductDependencies {
                verifier: authority.clone(),
                access: authority.clone(),
                clock: authority.clone(),
                policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
                uptime_clock: authority.clone(),
                agent_probe: Arc::new(StubAgentProbe::answering(false, false)),
            },
        );
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
        let conversations =
            Arc::new(LocalConversationStore::open(&private.join("metadata.sqlite3")).unwrap());
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
                SessionSnapshot {
                    id: session_id.clone(),
                    provider: provider.clone(),
                    provider_context: ProviderContext::Absent,
                    invocations: Vec::new(),
                    queue_history: Vec::new(),
                },
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
            .with_passive_read(Arc::new(RecordBinding), conversations)
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
                .with_passive_read(Arc::new(RecordBinding), repository)
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
        assert!(correlatable_invalid_request(
            r#"{"type":"req","id":"\ud800","method":"server.health","params":{}}"#
        )
        .is_none());
        assert!(correlatable_invalid_request(
            r#"{"type":"req","id":"a","id":"b","method":"mcp.callTool","params":{"nested":"\ud800"}}"#
        )
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
        };
        ordinary_send
            .send(response(
                "first",
                slots.clone().try_acquire_owned().unwrap(),
            ))
            .await
            .unwrap();
        let deliveries = Arc::new(WatchDeliveries::new());
        let writer = tokio::spawn(write_authenticated(
            sink,
            controls,
            refusals,
            ordinary,
            records,
            Duration::from_secs(1),
            deliveries.clone(),
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
        deliveries.close();
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
        let deliveries = Arc::new(WatchDeliveries::new());
        let writer = tokio::spawn(write_authenticated(
            sink,
            controls,
            refusals,
            ordinary,
            records,
            Duration::from_secs(1),
            deliveries.clone(),
        ));
        record_send
            .send(QueuedRecordResponse::new(QueuedResponse {
                message: WireResponse::record(record.clone()),
                _slot: slots.clone().try_acquire_owned().unwrap().into(),
                _record_work: Some(RecordReadLease::new(
                    record_capacity.clone().try_acquire_owned().unwrap(),
                )),
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
        deliveries.close();
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
        let (mut socket, mut peer) = test_socket(None);
        assert!(send_queued(
            Duration::from_secs(1),
            &mut socket,
            WireResponse::record("x".repeat(MAX_RECORD_RESPONSE_BYTES + 1)),
        )
        .await
        .is_err());
        assert!(peer.writing.try_recv().is_err());

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
            refusals,
            ordinary,
            records,
            Duration::from_secs(5),
            Arc::new(WatchDeliveries::new()),
        ));
        record_send
            .send(QueuedRecordResponse::new(QueuedResponse {
                message: WireResponse::record("{\"type\":\"res\"}".into()),
                _slot: slots.clone().try_acquire_owned().unwrap().into(),
                _record_work: None,
            }))
            .await
            .unwrap();
        peer.writing.recv().await.unwrap();
        control_send
            .send(ControlOutput::Response(Box::new(QueuedResponse {
                message: WireResponse::ordinary(success("control", &json!({}))),
                _slot: slots.clone().try_acquire_owned().unwrap().into(),
                _record_work: None,
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
                        "credential": "secret", "client": {"id": "test"}
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
                    r#"{{"type":"req","id":"auth","method":"session.authenticate","params":{{"minVersion":1,"maxVersion":1,"nonce":"{nonce}","credential":"wrong","credential":"secret","client":{{"id":"test"}}}}}}"#
                )
                .into(),
            )))
            .unwrap();
        let mut message = peer.message().await;
        if matches!(message, Message::Text(_)) {
            message = peer.message().await;
        }
        let Message::Close(Some(close)) = message else {
            panic!("close expected")
        };
        assert_eq!(close.code, 4001);
        task.await.unwrap();
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
