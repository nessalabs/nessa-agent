use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

use crate::agents::application::{AgentProbe, ReadAgentReadiness};
use crate::agents::domain::{AgentId, Readiness};
use crate::server::entrypoint::origin;

/// One agent, and what stands between it and running.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentReadinessView {
    /// The agent's name, as the interface knows it.
    id: &'static str,
    /// `ready`, `needs-authentication`, or `not-installed`.
    readiness: &'static str,
}

/// Every agent this server would be able to start.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentsReadinessView {
    agents: Vec<AgentReadinessView>,
}

/// The name this agent is known by on the wire and in the interface.
///
/// The domain has no opinion about this; a rename here is a wire change, not a
/// change to what an agent is.
fn agent_name(agent: AgentId) -> &'static str {
    match agent {
        AgentId::Claude => "claude",
    }
}

/// The name a readiness is reported under.
///
/// The wire has three names. A sign-in this machine could not determine is
/// reported as one that is needed: signing in is the one action that settles
/// the question either way, and it is better advice than silence. The
/// distinction survives in the domain for an interface that wants to say more.
fn readiness_name(readiness: Readiness) -> &'static str {
    match readiness {
        Readiness::Ready => "ready",
        Readiness::NeedsAuthentication | Readiness::AuthenticationUnknown => "needs-authentication",
        Readiness::NotInstalled => "not-installed",
    }
}

/// `GET /onboarding/agents`.
///
/// Deliberately unauthenticated. It answers the question asked while Nessa is
/// being set up — before there is a session, and before there is anything to
/// authenticate with — so requiring a session would make it unanswerable
/// exactly when it is needed.
///
/// What it discloses is bounded to make that safe: three names and three
/// states, no paths, no versions, no account, and never a credential. To
/// anything that can already reach this port, "an agent is installed here" is
/// not a secret worth a handshake.
pub(crate) async fn handle_http_agents(
    State(probe): State<Arc<dyn AgentProbe>>,
    headers: HeaderMap,
) -> Response {
    // Who asked is settled before the machine is touched. A page this server
    // does not trust gets its refusal without a single file, process, or
    // keychain being consulted on its behalf.
    let allowed = match allowed_origin(&headers) {
        Allowed::No => return StatusCode::FORBIDDEN.into_response(),
        allowed => allowed,
    };
    let Ok(agents) = tokio::task::spawn_blocking(move || read_readiness(probe.as_ref())).await
    else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let body = Json(AgentsReadinessView { agents });
    match allowed {
        Allowed::Cross(origin) => {
            let mut response = body.into_response();
            response
                .headers_mut()
                .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
            response
        }
        _ => body.into_response(),
    }
}

/// Ask the host about every agent, under its own thread.
///
/// Answering means metadata reads and, on macOS, spawning `security` and
/// waiting on it. That is a blocking OS call: run on a socket worker it would
/// hold the whole executor for as long as the machine takes to answer, so it
/// runs where blocking is what the thread is for.
fn read_readiness(probe: &dyn AgentProbe) -> Vec<AgentReadinessView> {
    ReadAgentReadiness { probe }
        .all()
        .into_iter()
        .map(|(agent, state)| AgentReadinessView {
            id: agent_name(agent),
            readiness: readiness_name(state),
        })
        .collect()
}

/// Who asked, and whether they may read the answer.
enum Allowed {
    /// No `Origin`: a native client or a command-line probe, not a page.
    Same,
    /// A page on an origin this server trusts, echoed back so the browser
    /// releases the response to it.
    Cross(HeaderValue),
    /// A page on any other origin.
    No,
}

/// Unauthenticated is not the same as open to every page on the internet.
///
/// Without an allow-origin header a browser refuses to hand this response to
/// the page that asked, which is what the app's own webview ran into: it lives
/// on `tauri://localhost` and every request from it is cross-origin. Echoing
/// the origin back where it is one this server already trusts for sessions
/// fixes that without turning "is Claude installed here" into something any
/// site you happen to visit can ask your machine.
fn allowed_origin(headers: &HeaderMap) -> Allowed {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return Allowed::Same;
    };
    if origin::is_trusted_ws_origin(origin) {
        Allowed::Cross(origin.clone())
    } else {
        Allowed::No
    }
}

#[cfg(test)]
#[path = "../../../tests/agents/http.rs"]
mod tests;
