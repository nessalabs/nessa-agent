use super::generated::{SessionCloseReason, SessionTermination};
use super::{state::ProductRouteState, wire::*};
use crate::protocol::{
    health_check_message, EventFrame, OutgoingMessage, RequestFrame, ResponseFrame,
    MAX_PAYLOAD_BYTES,
};
use axum::extract::ws::{CloseFrame, Message};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use nessa_auth::{
    application::{
        authorization::AuthorizeAction,
        credential_admin::{
            CredentialAdminError, IssueCredentialOutcome, IssueCredentialRequest,
            ListCredentialsRequest, RevokeCredentialRequest,
        },
        ports::{AccessError, CredentialEvidence, Decision},
        session::{AuthenticateSession, AuthenticatedSession},
    },
    domain::Action,
};
use serde_json::json;
use std::time::Duration;
use tokio::time::{interval, timeout, timeout_at, Instant, MissedTickBehavior};
use uuid::Uuid;

const PRODUCT_METHODS: [&str; 6] = [
    "auth.session",
    "server.health",
    "conversation.echo",
    "credential.issue",
    "credential.list",
    "credential.revoke",
];

/// Run one mandatory-authentication product session.
pub async fn handle_socket<S>(mut socket: S, state: ProductRouteState)
where
    S: Stream<Item = Result<Message, axum::Error>> + Sink<Message> + Unpin,
{
    let deadline = Instant::now() + state.settings.handshake_timeout;
    let nonce = Uuid::new_v4().to_string();
    // Wire timestamps use seconds. Round up from millisecond wall time so the
    // advertisement never truncates the authentication window. Only the shared
    // monotonic deadline below enforces it, including challenge delivery.
    let challenge_expires_at = state
        .clock
        .unix_milliseconds()
        .saturating_add(state.settings.handshake_timeout.as_millis() as u64)
        .div_ceil(1000);
    let challenge = SessionChallenge {
        min_version: PRODUCT_VERSION,
        max_version: PRODUCT_VERSION,
        nonce: nonce.clone(),
        expires_at: challenge_expires_at,
    };
    let challenge = match EventFrame::push("session.challenge", &challenge, 1, 0) {
        Ok(frame) => OutgoingMessage::Event(frame),
        Err(_) => return,
    };
    let authenticated = timeout_at(deadline, async {
        send(state.settings.write_timeout, &mut socket, challenge)
            .await
            .map_err(|_| (String::new(), "temporarily_unavailable"))?;
        receive_authentication(&mut socket, &state, &nonce, deadline).await
    })
    .await;
    let (request_id, session) = match authenticated {
        Ok(Ok(value)) => value,
        Ok(Err((request_id, code))) => {
            if code != "handshake_timeout" {
                let _ =
                    send_error(state.settings.write_timeout, &mut socket, &request_id, code).await;
            }
            let reason = match code {
                "protocol_incompatible" => SessionCloseReason::ProtocolIncompatible,
                "temporarily_unavailable" => SessionCloseReason::TemporaryUnavailable,
                "handshake_timeout" => SessionCloseReason::HandshakeTimeout,
                _ => SessionCloseReason::AuthenticationFailed,
            };
            close_session(state.settings.write_timeout, &mut socket, reason).await;
            return;
        }
        Err(_) => {
            close_session(
                state.settings.write_timeout,
                &mut socket,
                SessionCloseReason::HandshakeTimeout,
            )
            .await;
            return;
        }
    };

    let snapshot = match current_snapshot(&state, &session).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            close_session(
                state.settings.write_timeout,
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
    if send(state.settings.write_timeout, &mut socket, response)
        .await
        .is_err()
    {
        return;
    }

    run_authenticated(socket, state, session).await;
}

async fn receive_authentication<S>(
    socket: &mut S,
    state: &ProductRouteState,
    nonce: &str,
    deadline: Instant,
) -> Result<(String, AuthenticatedSession), (String, &'static str)>
where
    S: Stream<Item = Result<Message, axum::Error>> + Unpin,
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
    let frame: RequestFrame =
        serde_json::from_str(&text).map_err(|_| (String::new(), "unauthorized"))?;
    if frame.kind != "req"
        || frame.id.is_empty()
        || frame.id.len() > 256
        || frame.method != "session.authenticate"
    {
        return Err((frame.id, "authentication_required"));
    }
    let params: SessionAuthenticateParams =
        serde_json::from_value(frame.params).map_err(|_| (frame.id.clone(), "unauthorized"))?;
    if !params.supports_v1() {
        return Err((frame.id, "protocol_incompatible"));
    }
    if params.nonce != nonce || params.client.id.is_empty() || params.client.id.len() > 256 {
        return Err((frame.id, "unauthorized"));
    }
    let evidence = CredentialEvidence::new(params.credential.into_bytes())
        .map_err(|_| (frame.id.clone(), "unauthorized"))?;
    let session = AuthenticateSession {
        verifier: state.verifier.as_ref(),
        access: state.access.as_ref(),
        clock: state.clock.as_ref(),
    }
    .execute(&evidence, &state.audience)
    .await
    .map_err(|error| {
        (
            frame.id.clone(),
            if error == AccessError::Unavailable {
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

async fn run_authenticated<S>(
    mut socket: S,
    state: ProductRouteState,
    session: AuthenticatedSession,
) where
    S: Stream<Item = Result<Message, axum::Error>> + Sink<Message> + Unpin,
{
    let mut current_state = interval(state.settings.current_state_interval);
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
    loop {
        tokio::select! {
            _ = &mut expiry => {
                close_session(state.settings.write_timeout, &mut socket, SessionCloseReason::CredentialExpired).await;
                break;
            }
            _ = current_state.tick() => {
                if let Some(error) = current_session_error(&state, &session).await {
                    close_session(state.settings.write_timeout, &mut socket, close_reason(error)).await;
                    break;
                }
            }
            incoming = socket.next() => {
                let Some(Ok(message)) = incoming else { break };
                let Message::Text(text) = message else {
                    if matches!(message, Message::Close(_)) { break; }
                    continue;
                };
                if text.len() > MAX_PAYLOAD_BYTES as usize { break; }
                let frame: RequestFrame = match serde_json::from_str(&text) {
                    Ok(frame) => frame,
                    Err(_) => {
                        let Some(response) = correlatable_invalid_request(&text) else { continue };
                        if let Some(error) = current_session_error(&state, &session).await {
                            close_session(state.settings.write_timeout, &mut socket, close_reason(error)).await;
                            break;
                        }
                        if send(state.settings.write_timeout, &mut socket, response).await.is_err() { break; }
                        continue;
                    },
                };
                if let Some(error) = current_session_error(&state, &session).await {
                    close_session(state.settings.write_timeout, &mut socket, close_reason(error)).await;
                    break;
                }
                // Admission authorizes one operation against committed state.
                // Its response may finish after revocation; the next request
                // and idle liveness check observe the new revision.
                let response = dispatch(&state, &session, frame).await;
                if send(state.settings.write_timeout, &mut socket, response).await.is_err() { break; }
            }
        }
    }
}

async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    if current_session_error(state, session).await.is_some() {
        return failure(&frame.id, "unauthorized");
    }
    if frame.kind != "req" || frame.id.is_empty() || frame.id.len() > 256 {
        return failure(&frame.id, "invalid_request");
    }
    if frame.method == "session.authenticate" {
        return failure(&frame.id, "already_authenticated");
    }
    if frame.method == "auth.session" {
        if frame.params != json!({}) {
            return failure(&frame.id, "invalid_request");
        }
        let snapshot = match current_snapshot(state, session).await {
            Ok(snapshot) => snapshot,
            Err(_) => return failure(&frame.id, "unauthorized"),
        };
        return success(&frame.id, &session_ready(state, session, &snapshot));
    }
    let action_name = match action_for_method(&frame.method) {
        Some(action) => action,
        _ => return failure(&frame.id, "unknown_method"),
    };
    let authorization = authorize(state, session, action_name).await;
    match authorization {
        Ok(Decision::Allow) => dispatch_authorized(state, session, frame).await,
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
        "conversation.echo" => {
            let params: crate::protocol::EchoParams = match serde_json::from_value(frame.params) {
                Ok(params) => params,
                Err(_) => return failure(&frame.id, "invalid_request"),
            };
            crate::protocol::echo_message(&frame.id, params.text)
                .unwrap_or_else(|_| failure(&frame.id, "internal_error"))
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
                matches!(grant.action.as_str(), "server.read" | "conversation.write")
                    && grant.resource.organization_id == context.organization_id().as_str()
                    && grant.resource.id == state.gateway_id().as_str()
            });
            if params.request_id.is_empty()
                || params.request_id.len() > 200
                || params.membership.organization_id != context.organization_id().as_str()
                || params.membership.principal_id != params.principal.id
                || !matches!(
                    params.membership.role,
                    nessa_auth::application::dto::MembershipRoleDto::Member
                )
                || !matches!(
                    params.membership.state,
                    nessa_auth::application::dto::MembershipStateDto::Active
                )
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
                Ok(IssueCredentialOutcome::Issued { metadata, evidence }) => {
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
                Ok(IssueCredentialOutcome::ExistingSecretUnavailable { metadata }) => success(
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
            let target_id = match nessa_auth::domain::CredentialId::new(&params.credential_id) {
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
                Ok(revision) => success(
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

fn success<T: serde::Serialize>(request_id: &str, payload: &T) -> OutgoingMessage {
    ResponseFrame::success(request_id, payload)
        .map(OutgoingMessage::Response)
        .unwrap_or_else(|_| failure(request_id, "internal_error"))
}

fn action_for_method(method: &str) -> Option<&'static str> {
    match method {
        "server.health" => Some("server.read"),
        "conversation.echo" => Some("conversation.write"),
        "credential.issue" | "credential.list" | "credential.revoke" => Some("credential.manage"),
        _ => None,
    }
}

fn correlatable_invalid_request(text: &str) -> Option<OutgoingMessage> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
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
    snapshot: &nessa_auth::application::ports::AccessSnapshot,
) -> SessionReady {
    let grants = snapshot
        .credential
        .grants()
        .iter()
        .map(|grant| nessa_auth::application::dto::CredentialGrantDto {
            action: grant.action().as_str().to_owned(),
            resource: nessa_auth::application::dto::ResourceDto {
                organization_id: grant.resource().organization_id().as_str().to_owned(),
                id: grant.resource().id().as_str().to_owned(),
            },
        })
        .collect();
    SessionReady::from_session(
        state.gateway_id().as_str(),
        session,
        grants,
        PRODUCT_METHODS
            .iter()
            .map(|method| (*method).to_owned())
            .collect(),
    )
}

async fn current_snapshot(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
) -> Result<nessa_auth::application::ports::AccessSnapshot, AccessError> {
    nessa_auth::application::session::ReadCurrentSession {
        access: state.access.as_ref(),
        clock: state.clock.as_ref(),
    }
    .execute(session)
    .await
}

async fn current_session_error(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
) -> Option<AccessError> {
    current_snapshot(state, session).await.err()
}

fn failure(request_id: &str, code: &str) -> OutgoingMessage {
    OutgoingMessage::Response(ResponseFrame::failure(request_id, code, code))
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
    let text = message.to_wire_text().map_err(|_| ())?;
    timeout(write_timeout, socket.send(Message::Text(text.into())))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
}

fn close_reason(error: AccessError) -> SessionCloseReason {
    match error {
        AccessError::CredentialRevoked => SessionCloseReason::CredentialRevoked,
        AccessError::CredentialExpired => SessionCloseReason::CredentialExpired,
        AccessError::Unavailable => SessionCloseReason::TemporaryUnavailable,
        _ => SessionCloseReason::AuthorizationLost,
    }
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
mod tests {
    use super::*;

    #[test]
    fn session_termination_is_typed_bounded_and_classifies_authority_failures() {
        for (error, expected, retryable) in [
            (AccessError::CredentialRevoked, "credential_revoked", false),
            (AccessError::CredentialExpired, "credential_expired", false),
            (AccessError::InactiveMembership, "authorization_lost", false),
            (AccessError::Unavailable, "temporary_unavailable", true),
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

    use crate::{app::ports::Clock as UptimeClock, product::ProductDependencies};
    use nessa_auth::{
        adapters::cedar::CedarPolicyEvaluator,
        application::ports::{
            AccessReader, AccessSnapshot, Clock, CredentialVerifier, PortFuture, VerifiedCredential,
        },
        domain::{
            AudienceId, Credential, CredentialId, Grant, Membership, MembershipId, MembershipRole,
            MembershipStatus, OrganizationId, PrincipalId, Resource, ResourceId,
        },
    };
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    };

    struct Authority {
        snapshot: Mutex<AccessSnapshot>,
        now: AtomicU64,
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
                    expires_at: Some(200),
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
    impl nessa_auth::application::credential_admin::CredentialAdmin for RejectingAdmin {
        fn issue<'a>(
            &'a self,
            _: IssueCredentialRequest,
        ) -> PortFuture<'a, IssueCredentialOutcome, CredentialAdminError> {
            Box::pin(async { Err(self.0) })
        }
        fn list<'a>(
            &'a self,
            _: ListCredentialsRequest,
        ) -> PortFuture<
            'a,
            Vec<nessa_auth::application::dto::CredentialMetadataDto>,
            CredentialAdminError,
        > {
            Box::pin(async { Err(self.0) })
        }
        fn revoke<'a>(
            &'a self,
            _: RevokeCredentialRequest,
        ) -> PortFuture<'a, u64, CredentialAdminError> {
            Box::pin(async { Err(self.0) })
        }
    }

    fn issue_params() -> serde_json::Value {
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
    impl nessa_auth::application::ports::PolicyEvaluator for UnavailablePolicy {
        fn evaluate(
            &self,
            _: &nessa_auth::domain::AuthContext,
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
                    .revoke(100)
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
            request("unknown", "conversation.echo"),
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

    #[tokio::test]
    async fn session_ready_reports_current_restrictions_and_registered_methods() {
        let (state, _) = fixture(MembershipRole::Member);
        let session = authenticate(&state).await;
        let snapshot = current_snapshot(&state, &session).await.unwrap();
        let value = serde_json::to_value(session_ready(&state, &session, &snapshot)).unwrap();
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
        incoming: tokio::sync::mpsc::UnboundedReceiver<Result<Message, axum::Error>>,
        outgoing: tokio::sync::mpsc::UnboundedSender<Message>,
        writing: tokio::sync::mpsc::UnboundedSender<()>,
        gate: Option<tokio::sync::oneshot::Receiver<()>>,
        pending: Vec<Message>,
    }
    impl Stream for TestSocket {
        type Item = Result<Message, axum::Error>;
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
        input: tokio::sync::mpsc::UnboundedSender<Result<Message, axum::Error>>,
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
        state.settings.handshake_timeout = Duration::from_secs(1);
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(handle_socket(socket, state));
        let Message::Text(challenge) = peer.message().await else {
            panic!("challenge expected")
        };
        let challenge: serde_json::Value = serde_json::from_str(&challenge).unwrap();
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
    async fn elapsed_handshake_closes_with_retryable_timeout() {
        let (mut state, _) = fixture(MembershipRole::Member);
        state.settings.handshake_timeout = Duration::from_secs(1);
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(handle_socket(socket, state));
        assert!(matches!(peer.message().await, Message::Text(_)));
        tokio::time::advance(Duration::from_millis(1001)).await;
        let Message::Close(Some(close)) = peer.message().await else {
            panic!("close expected")
        };
        assert_eq!(close.code, 4006);
        let reason: serde_json::Value = serde_json::from_str(&close.reason).unwrap();
        assert_eq!(reason["code"], "handshake_timeout");
        assert_eq!(reason["retryable"], true);
        task.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn challenge_write_consumes_the_same_handshake_budget() {
        let (mut state, _) = fixture(MembershipRole::Member);
        state.settings.handshake_timeout = Duration::from_secs(1);
        state.settings.write_timeout = Duration::from_secs(5);
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
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["id"], id);
        assert_eq!(value["ok"], true);
    }
    fn revoke(authority: &Authority) {
        let mut snapshot = authority.snapshot.lock().unwrap();
        snapshot.credential.revoke(100).unwrap();
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
    impl nessa_auth::application::ports::PolicyEvaluator for RevokeAfterAdmission {
        fn evaluate(
            &self,
            context: &nessa_auth::domain::AuthContext,
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
}
