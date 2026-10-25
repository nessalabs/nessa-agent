//! The GitHub sign-in's wire commands: `github.status`, `github.connect`,
//! `github.cancel` and `github.disconnect`, translated into
//! [`GithubConnections`]. The socket has already asked Cedar for
//! `credential.manage` before these are reached. Every answer is the
//! sign-in as it stands, never its token.
use super::{
    socket::{failure, success},
    state::ProductRouteState,
};
use crate::github::application::{GithubCaller, GithubConnections, GithubError};
use crate::github::domain::{ConnectEnd, GithubStatus};
use nessa_auth::application::session::AuthenticatedSession;
use nessa_protocol::product::generated::{
    GithubAccount, GithubCode, GithubConnectEnd, GithubConnectState, GithubConnection,
    GithubErrorCode,
};
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};
use serde_json::json;

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    let Some(github) = state.github.as_ref() else {
        return failure(&frame.id, GithubErrorCode::GithubNotConfigured.as_str());
    };
    if frame.params != json!({}) {
        return failure(&frame.id, "invalid_request");
    }
    let caller = caller(session);
    let answer = match frame.method.as_str() {
        "github.status" => Ok(github.connections.status()),
        "github.connect" => github.connections.connect(caller).await,
        "github.cancel" => github.connections.cancel(caller).await,
        "github.disconnect" => github.connections.disconnect(caller).await,
        _ => return failure(&frame.id, "unknown_method"),
    };
    match answer {
        Ok(status) => success(&frame.id, &connection(status, &github.install_url)),
        Err(error) => failure(&frame.id, code(error).as_str()),
    }
}

/// What the github methods are given: the sign-in, and where its repositories
/// are chosen.
pub struct GithubRoutes {
    pub connections: std::sync::Arc<GithubConnections>,
    pub install_url: String,
}

/// The authenticated caller, as each change's records name it.
fn caller(session: &AuthenticatedSession) -> GithubCaller {
    let context = session.context();
    GithubCaller {
        organization_id: context.organization_id().as_str().to_owned(),
        principal_id: context.principal_id().as_str().to_owned(),
        credential_id: context.credential_id().as_str().to_owned(),
    }
}

fn code(error: GithubError) -> GithubErrorCode {
    match error {
        GithubError::Unreachable => GithubErrorCode::GithubUnreachable,
        GithubError::Rejected => GithubErrorCode::GithubRejected,
        GithubError::StoreUnavailable => GithubErrorCode::GithubStorageUnavailable,
        GithubError::AuditUnavailable => GithubErrorCode::GithubAuditUnavailable,
    }
}

fn end(end: ConnectEnd) -> GithubConnectEnd {
    match end {
        ConnectEnd::Expired => GithubConnectEnd::Expired,
        ConnectEnd::Denied => GithubConnectEnd::Denied,
        ConnectEnd::Cancelled => GithubConnectEnd::Cancelled,
        ConnectEnd::Rejected => GithubConnectEnd::Rejected,
        ConnectEnd::Unreachable => GithubConnectEnd::Unreachable,
        ConnectEnd::StoreUnavailable => GithubConnectEnd::StoreUnavailable,
        ConnectEnd::AuditUnavailable => GithubConnectEnd::AuditUnavailable,
    }
}

pub(super) fn connection(status: GithubStatus, install_url: &str) -> GithubConnection {
    let mut wire = GithubConnection {
        state: GithubConnectState::SignedOut,
        account: None,
        code: None,
        last: None,
        install_url: install_url.to_owned(),
    };
    match status {
        GithubStatus::SignedOut { last } => wire.last = last.map(end),
        GithubStatus::Connecting {
            user_code,
            verification_uri,
            expires_in_seconds,
        } => {
            wire.state = GithubConnectState::Connecting;
            wire.code = Some(GithubCode {
                user_code,
                verification_uri,
                expires_in_seconds,
            });
        }
        GithubStatus::Connected { account } => {
            wire.state = GithubConnectState::Connected;
            wire.account = Some(GithubAccount {
                id: account.id,
                login: account.login,
            });
        }
    }
    wire
}
