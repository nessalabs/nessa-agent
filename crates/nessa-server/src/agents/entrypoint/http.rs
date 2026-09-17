use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

use crate::agents::adapters::LocalAgentProbe;
use crate::agents::application::ReadAgentReadiness;
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
pub(crate) async fn handle_http_agents(headers: HeaderMap) -> Response {
    let probe = LocalAgentProbe;
    let readiness = ReadAgentReadiness { probe: &probe };
    let body = Json(AgentsReadinessView {
        agents: readiness
            .all()
            .into_iter()
            .map(|(agent, state)| AgentReadinessView {
                id: agent.as_str(),
                readiness: state.as_str(),
            })
            .collect(),
    });
    match allowed_origin(&headers) {
        Allowed::Same => body.into_response(),
        Allowed::Cross(origin) => {
            let mut response = body.into_response();
            response
                .headers_mut()
                .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
            response
        }
        Allowed::No => StatusCode::FORBIDDEN.into_response(),
    }
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
