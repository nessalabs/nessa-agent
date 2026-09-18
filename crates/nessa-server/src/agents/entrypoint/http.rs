use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

use crate::agents::application::{ReadingFailure, SharedAgentReadiness};
use crate::agents::domain::{AgentId, Readiness};
use crate::server::entrypoint::origin;

/// One agent, and what stands between it and running.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentReadinessView {
    /// The agent's name, as the interface knows it.
    id: &'static str,
    /// `ready`, `needs-authentication`, `not-installed`, or `not-configured`.
    readiness: &'static str,
}

/// Every agent Nessa has an adapter for, each with what stands between it and
/// running here — including the ones this server is not configured for, which
/// are reported as exactly that rather than left out.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentsReadinessView {
    agents: Vec<AgentReadinessView>,
}

/// The name a readiness is reported under.
///
/// The wire has four names. A sign-in this machine could not determine is
/// reported as one that is needed: signing in is the one action that settles
/// the question either way, and it is better advice than silence. The
/// distinction survives in the domain for an interface that wants to say more.
///
/// An agent this server is not configured for keeps its own name rather than
/// joining "not installed": the one thing a person must not be told is to go
/// and install what is already sitting on their machine.
fn readiness_name(readiness: Readiness) -> &'static str {
    match readiness {
        Readiness::Ready => "ready",
        Readiness::NeedsAuthentication | Readiness::AuthenticationUnknown => "needs-authentication",
        Readiness::NotInstalled => "not-installed",
        Readiness::NotConfigured => "not-configured",
    }
}

/// `GET /onboarding/agents`.
///
/// Deliberately unauthenticated. It answers the question asked while Nessa is
/// being set up — before there is a session, and before there is anything to
/// authenticate with — so requiring a session would make it unanswerable
/// exactly when it is needed.
///
/// What it discloses is bounded to make that safe: a name and a state per
/// agent, no paths, no versions, no account, and never a credential. To
/// anything that can already reach this port, "an agent is installed here" is
/// not a secret worth a handshake.
/// What this costs is bounded by [`SharedAgentReadiness`], which runs one probe
/// however many callers are asking and stops waiting for it after a deadline. A
/// reading that did not arrive is reported as no reading — see [`declined`] —
/// rather than as an agent that is missing or signed out.
pub(crate) async fn handle_http_agents(
    State(host): State<Arc<SharedAgentReadiness>>,
    headers: HeaderMap,
) -> Response {
    // Who asked is settled before the machine is touched. A page this server
    // does not trust gets its refusal without a single file, process, or
    // keychain being consulted on its behalf.
    let allowed = match allowed_origin(&headers) {
        Allowed::No => return StatusCode::FORBIDDEN.into_response(),
        allowed => allowed,
    };
    let mut response = match host.read().await {
        Ok(agents) => Json(AgentsReadinessView {
            agents: view(agents),
        })
        .into_response(),
        Err(ReadingFailure::Undetermined) => declined(),
        // The asking came apart rather than ran long. That is this server
        // failing, not this server declining, and it is reported as one.
        Err(ReadingFailure::Lost) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    // Attached to whatever came out, including a refusal to answer: a browser
    // that is not allowed to read the status cannot tell "no answer" from "no
    // gateway", and the page is entitled to know which it met.
    if let Allowed::Cross(origin) = allowed {
        response
            .headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    }
    response
}

/// This server was asked and would not say.
///
/// A 503 rather than a 200 carrying some readiness, because there is no honest
/// readiness to carry. Every name the wire has is a *claim* — `not-installed`
/// and `needs-authentication` tell the person to go and do something,
/// `not-configured` tells them this build will not run it — and this server did
/// not find any of them out; it declined to ask.
/// Saying so as a status keeps "could not determine" from being dressed up as a
/// fact, and setup already reports a gateway with no answer as a gateway with no
/// answer. Retrying is the right response, so the person's "check again" is too.
fn declined() -> Response {
    StatusCode::SERVICE_UNAVAILABLE.into_response()
}

/// Name each agent and its state the way the wire does.
fn view(agents: Vec<(AgentId, Readiness)>) -> Vec<AgentReadinessView> {
    agents
        .into_iter()
        .map(|(agent, state)| AgentReadinessView {
            id: agent.name(),
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
