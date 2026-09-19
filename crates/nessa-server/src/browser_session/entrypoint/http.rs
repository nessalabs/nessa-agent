use crate::browser_session::{
    application::{invalidation_reason, BrowserSessionVerifier, ReadBrowserSession, SignIn},
    domain::value_objects::RemovalReason,
};
use crate::product::ProductRouteState;
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use nessa_auth::application::{
    ports::{AccessError, SessionEvidence},
    session::{AuthenticateSession, ResumeSession},
};
use serde::Deserialize;
use std::fmt::Write;
use tokio::sync::oneshot;

pub(crate) fn encode_session_id(bytes: [u8; 32]) -> String {
    let mut id = String::with_capacity(64);
    for byte in bytes {
        write!(&mut id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    id
}

fn new_session_id() -> Result<String, AccessError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| AccessError::Unavailable)?;
    Ok(encode_session_id(bytes))
}

const COOKIE: &str = "__Host-nessa-session";
const LOCAL_COOKIE: &str = "nessa-local-session";

fn cookie_settings(headers: &HeaderMap) -> (&'static str, &'static str) {
    if headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("http://"))
    {
        (LOCAL_COOKIE, "")
    } else {
        (COOKIE, "; Secure")
    }
}

pub(crate) fn origin(headers: &HeaderMap, allow_http: bool) -> Option<&str> {
    let value = headers.get(header::ORIGIN)?.to_str().ok()?;
    ((value.starts_with("https://")
        || (allow_http
            && (value.starts_with("http://127.0.0.1:")
                || value == "http://127.0.0.1"
                || value.starts_with("http://[::1]:")
                || value == "http://[::1]")))
        && crate::server::entrypoint::origin::is_trusted_origin_value(value))
    .then_some(value)
}
pub(crate) fn cookie(headers: &HeaderMap) -> Option<&str> {
    let mut matches = headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .filter(|(name, _)| *name == cookie_settings(headers).0);
    let (_, value) = matches.next()?;
    if matches.next().is_some()
        || value.len() != 64
        || !value.bytes().all(|c| c.is_ascii_hexdigit())
    {
        return None;
    }
    Some(value)
}
fn allowed(headers: &HeaderMap, state: &ProductRouteState) -> bool {
    origin(headers, state.browser_http_allowed).is_some()
        && headers.get("x-nessa-browser").is_some_and(|v| v == "1")
}
fn response(status: StatusCode, value: Option<String>) -> Response {
    let mut response = status.into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    if let Some(value) = value {
        response
            .headers_mut()
            .insert(header::SET_COOKIE, value.parse().unwrap());
    }
    response
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Login {
    token: String,
}

async fn operation_result<T>(
    deadline: std::time::Duration,
    result: oneshot::Receiver<Result<T, AccessError>>,
) -> Result<T, AccessError> {
    tokio::time::timeout(deadline, result)
        .await
        .map_err(|_| AccessError::Unavailable)?
        .map_err(|_| AccessError::Unavailable)?
}

pub async fn login(
    State(state): State<ProductRouteState>,
    headers: HeaderMap,
    Json(body): Json<Login>,
) -> Response {
    if !allowed(&headers, &state) {
        return response(StatusCode::FORBIDDEN, None);
    }
    let Ok(permit) = state.requests.clone().try_acquire_owned() else {
        return response(StatusCode::TOO_MANY_REQUESTS, None);
    };
    let Some(store) = &state.browser_sessions else {
        return response(StatusCode::SERVICE_UNAVAILABLE, None);
    };
    let Ok(id) = new_session_id() else {
        return response(StatusCode::SERVICE_UNAVAILABLE, None);
    };
    let request_origin = origin(&headers, state.browser_http_allowed)
        .expect("allowed browser request has an origin")
        .to_owned();
    let prior = cookie(&headers).map(str::to_owned);
    let operation_state = state.clone();
    let operation_store = store.clone();
    let operation_id = id.clone();
    let (reply, result) = oneshot::channel();
    tokio::spawn(async move {
        let _permit = permit;
        let outcome = async {
            let now = operation_state.clock.unix_seconds();
            let prior_id = match prior.as_deref() {
                Some(id) => ReadBrowserSession {
                    store: operation_store.as_ref(),
                }
                .execute(id, now)
                .await?
                .filter(|session| session.origin() == request_origin)
                .map(|_| id.to_owned()),
                None => None,
            };
            let (lifetime, replaced) = SignIn {
                authentication: AuthenticateSession {
                    verifier: operation_state.verifier.as_ref(),
                    access: operation_state.access.as_ref(),
                    clock: operation_state.clock.as_ref(),
                },
                store: operation_store.as_ref(),
                audience: &operation_state.audience,
            }
            .execute(
                body.token,
                request_origin,
                operation_id.clone(),
                prior_id.as_deref(),
            )
            .await?;
            Ok::<_, AccessError>((lifetime, replaced))
        }
        .await;
        let (lifetime, prior) = match outcome {
            Ok(value) => value,
            Err(error) => {
                let _ = reply.send(Err(error));
                return;
            }
        };
        if let Err(Ok(_)) = reply.send(Ok(lifetime)) {
            let cleanup_at = match operation_store.get(operation_id.clone()).await {
                Ok(Some(session)) => prior
                    .as_ref()
                    .map(|(_, prior)| prior.renewed_at())
                    .unwrap_or(0)
                    .max(operation_state.clock.unix_seconds())
                    .max(session.renewed_at()),
                Ok(None) => return,
                Err(error) => {
                    tracing::error!(?error, "failed to inspect an abandoned browser login");
                    return;
                }
            };
            if let Err(error) = operation_store
                .abandon_login(operation_id, prior, cleanup_at)
                .await
            {
                tracing::error!(?error, "failed to reclaim an abandoned browser login");
            }
        }
    });
    let result = operation_result(state.settings.handshake_timeout(), result).await;
    let lifetime = match result {
        Ok(lifetime) => lifetime,
        Err(AccessError::Unavailable) => return response(StatusCode::SERVICE_UNAVAILABLE, None),
        _ => return response(StatusCode::UNAUTHORIZED, None),
    };
    let (name, secure) = cookie_settings(&headers);
    response(
        StatusCode::NO_CONTENT,
        Some(format!(
            "{name}={id}; Path=/; HttpOnly{secure}; SameSite=Strict; Max-Age={}",
            lifetime
        )),
    )
}
pub async fn check(State(state): State<ProductRouteState>, headers: HeaderMap) -> Response {
    if !allowed(&headers, &state) {
        return response(StatusCode::FORBIDDEN, None);
    }
    let Some(store) = &state.browser_sessions else {
        return response(StatusCode::SERVICE_UNAVAILABLE, None);
    };
    let Some(id) = cookie(&headers) else {
        return response(StatusCode::UNAUTHORIZED, None);
    };
    let Ok(permit) = state.requests.clone().try_acquire_owned() else {
        return response(StatusCode::SERVICE_UNAVAILABLE, None);
    };
    let operation_state = state.clone();
    let operation_store = store.clone();
    let id = id.to_owned();
    let operation_id = id.clone();
    let request_origin = origin(&headers, state.browser_http_allowed)
        .expect("allowed browser request has an origin")
        .to_owned();
    let (reply, result) = oneshot::channel();
    tokio::spawn(async move {
        let _permit = permit;
        let outcome = async {
            let now = operation_state.clock.unix_seconds();
            let Some(_session) = (ReadBrowserSession {
                store: operation_store.as_ref(),
            })
            .execute(&operation_id, now)
            .await?
            .filter(|session| session.origin() == request_origin) else {
                return Ok(None);
            };
            let evidence = SessionEvidence::new(operation_id.as_bytes().to_vec())?;
            let verifier = BrowserSessionVerifier {
                store: operation_store.as_ref(),
                expected_origin: &request_origin,
                now,
            };
            let identity = match (ResumeSession {
                verifier: &verifier,
                access: operation_state.access.as_ref(),
                clock: operation_state.clock.as_ref(),
            })
            .execute(&evidence, &operation_state.audience)
            .await
            {
                Ok((identity, _)) => identity,
                Err(error) => {
                    if let Some(reason) = invalidation_reason(error) {
                        operation_store
                            .remove(
                                operation_id.clone(),
                                operation_state.clock.unix_seconds(),
                                reason,
                                None,
                            )
                            .await?;
                    }
                    return Err(error);
                }
            };
            let renewal_now = operation_state.clock.unix_seconds();
            let renewed = operation_store
                .renew(
                    operation_id,
                    renewal_now,
                    identity.context().credential_id().clone(),
                )
                .await?;
            Ok(Some(renewed))
        }
        .await;
        let _ = reply.send(outcome);
    });
    let result = operation_result(state.settings.handshake_timeout(), result).await;
    match result {
        Ok(Some(renewed)) => {
            let now = state.clock.unix_seconds();
            let (name, secure) = cookie_settings(&headers);
            response(
                StatusCode::NO_CONTENT,
                Some(format!(
                    "{name}={id}; Path=/; HttpOnly{secure}; SameSite=Strict; Max-Age={}",
                    renewed.idle_expires_at().saturating_sub(now)
                )),
            )
        }
        Ok(None) => response(StatusCode::UNAUTHORIZED, None),
        Err(AccessError::Unavailable) => response(StatusCode::SERVICE_UNAVAILABLE, None),
        Err(_) => response(StatusCode::UNAUTHORIZED, None),
    }
}
pub async fn logout(State(state): State<ProductRouteState>, headers: HeaderMap) -> Response {
    if !allowed(&headers, &state) {
        return response(StatusCode::FORBIDDEN, None);
    }
    let Ok(permit) = state.requests.clone().try_acquire_owned() else {
        return response(StatusCode::SERVICE_UNAVAILABLE, None);
    };
    let operation_state = state.clone();
    let operation_store = state.browser_sessions.clone();
    let id = cookie(&headers).map(str::to_owned);
    let request_origin = origin(&headers, state.browser_http_allowed)
        .expect("allowed browser request has an origin")
        .to_owned();
    let (reply, removal) = oneshot::channel();
    tokio::spawn(async move {
        let _permit = permit;
        let outcome = async {
            if let (Some(store), Some(id)) = (&operation_store, id.as_deref()) {
                if let Some(_session) = (ReadBrowserSession {
                    store: store.as_ref(),
                })
                .execute(id, operation_state.clock.unix_seconds())
                .await?
                .filter(|session| session.origin() == request_origin)
                {
                    let evidence = SessionEvidence::new(id.as_bytes().to_vec())?;
                    let verifier = BrowserSessionVerifier {
                        store: store.as_ref(),
                        expected_origin: &request_origin,
                        now: operation_state.clock.unix_seconds(),
                    };
                    let (reason, initiator) = match (ResumeSession {
                        verifier: &verifier,
                        access: operation_state.access.as_ref(),
                        clock: operation_state.clock.as_ref(),
                    })
                    .execute(&evidence, &operation_state.audience)
                    .await
                    {
                        Ok((identity, _)) => (
                            RemovalReason::SignOut,
                            Some(identity.context().credential_id().clone()),
                        ),
                        Err(error) => match invalidation_reason(error) {
                            Some(reason) => (reason, None),
                            None => return Err(error),
                        },
                    };
                    store
                        .remove(
                            id.to_owned(),
                            operation_state.clock.unix_seconds(),
                            reason,
                            initiator,
                        )
                        .await?;
                }
            }
            Ok::<(), AccessError>(())
        }
        .await;
        let _ = reply.send(outcome);
    });
    if operation_result(state.settings.handshake_timeout(), removal)
        .await
        .is_err()
    {
        return response(StatusCode::SERVICE_UNAVAILABLE, None);
    }
    let (name, secure) = cookie_settings(&headers);
    response(
        StatusCode::NO_CONTENT,
        Some(format!(
            "{name}=; Path=/; HttpOnly{secure}; SameSite=Strict; Max-Age=0"
        )),
    )
}
