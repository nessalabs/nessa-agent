//! The saved-login methods (issue #1260): `logins.status`, `logins.save`,
//! `logins.remove` and `logins.setDevice`, each admitted by Cedar's
//! `credential.manage`, translated into [`SavedLoginsService`]. No answer
//! carries a login.
use super::{
    socket::{failure, success},
    state::ProductRouteState,
};
use crate::logins::application::{LoginCaller, LoginsError, LoginsStatus};
use nessa_auth::application::session::AuthenticatedSession;
use nessa_protocol::agents::AgentId;
use nessa_protocol::lease::Login;
use nessa_protocol::product::generated::{
    LoginAgent, LoginsErrorCode, LoginsRemoveParams, LoginsSaveParams, LoginsSetDeviceParams,
    LoginsStatus as WireStatus, SavedLogin,
};
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};
use nessa_sdk::domain::agent_execution::leases::PeerEnvironment;
use serde_json::json;

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    let Some(logins) = state.logins.as_ref() else {
        return failure(&frame.id, LoginsErrorCode::LoginsNotConfigured.as_str());
    };
    let caller = caller(session);
    let answer = match frame.method.as_str() {
        "logins.status" => {
            if frame.params != json!({}) {
                return failure(&frame.id, "invalid_request");
            }
            logins.status()
        }
        "logins.save" => {
            let Ok(params) = serde_json::from_value::<LoginsSaveParams>(frame.params.clone())
            else {
                return failure(&frame.id, "invalid_request");
            };
            let Ok(login) = Login::new(params.login) else {
                return failure(&frame.id, "invalid_request");
            };
            logins.save(caller, agent(params.agent), login).await
        }
        "logins.remove" => {
            let Ok(params) = serde_json::from_value::<LoginsRemoveParams>(frame.params.clone())
            else {
                return failure(&frame.id, "invalid_request");
            };
            logins.remove(caller, agent(params.agent)).await
        }
        "logins.setDevice" => {
            let Ok(params) = serde_json::from_value::<LoginsSetDeviceParams>(frame.params.clone())
            else {
                return failure(&frame.id, "invalid_request");
            };
            let Ok(peer) = PeerEnvironment::parse(&params.environment) else {
                return failure(&frame.id, "invalid_request");
            };
            logins.set_device(caller, peer, params.on).await
        }
        _ => return failure(&frame.id, "unknown_method"),
    };
    match answer {
        Ok(status) => success(&frame.id, &wire(&status)),
        Err(error) => failure(&frame.id, code(error).as_str()),
    }
}

fn agent(agent: LoginAgent) -> AgentId {
    match agent {
        LoginAgent::Claude => AgentId::Claude,
        LoginAgent::Codex => AgentId::Codex,
        LoginAgent::Opencode => AgentId::Opencode,
    }
}

fn wire_agent(agent: AgentId) -> LoginAgent {
    match agent {
        AgentId::Claude => LoginAgent::Claude,
        AgentId::Codex => LoginAgent::Codex,
        AgentId::Opencode => LoginAgent::Opencode,
    }
}

fn wire(status: &LoginsStatus) -> WireStatus {
    WireStatus {
        logins: AgentId::ALL
            .iter()
            .map(|agent| SavedLogin {
                agent: wire_agent(*agent),
                saved: status.saved.get(agent).copied().unwrap_or(false),
            })
            .collect(),
        devices_on: status
            .devices_on
            .iter()
            .map(PeerEnvironment::name)
            .collect(),
    }
}

fn caller(session: &AuthenticatedSession) -> LoginCaller {
    let context = session.context();
    LoginCaller {
        organization_id: context.organization_id().as_str().to_owned(),
        principal_id: context.principal_id().as_str().to_owned(),
        credential_id: context.credential_id().as_str().to_owned(),
    }
}

fn code(error: LoginsError) -> LoginsErrorCode {
    match error {
        LoginsError::VaultLocked => LoginsErrorCode::VaultLocked,
        LoginsError::StorageUnavailable => LoginsErrorCode::LoginsStorageUnavailable,
        LoginsError::AuditUnavailable => LoginsErrorCode::LoginsAuditUnavailable,
        LoginsError::TooManyDevices => LoginsErrorCode::LoginsTooManyDevices,
    }
}
