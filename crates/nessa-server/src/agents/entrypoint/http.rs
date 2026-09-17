use axum::Json;
use serde::Serialize;

use crate::agents::adapters::LocalAgentProbe;
use crate::agents::application::ReadAgentReadiness;

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
pub(crate) async fn handle_http_agents() -> Json<AgentsReadinessView> {
    let probe = LocalAgentProbe;
    let readiness = ReadAgentReadiness { probe: &probe };
    Json(AgentsReadinessView {
        agents: readiness
            .all()
            .into_iter()
            .map(|(agent, state)| AgentReadinessView {
                id: agent.as_str(),
                readiness: state.as_str(),
            })
            .collect(),
    })
}
