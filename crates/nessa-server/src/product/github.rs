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
    GithubErrorCode, GithubLiveEvents, GithubLiveEventsParams,
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
    // The command may wait behind another GitHub request while this connection
    // locks and unlocks. Carry the admitted unlock across that wait.
    let admitted_generation = state
        .vault
        .as_ref()
        .map(|vault| vault.vault.unlocked_generation());
    if admitted_generation == Some(None) {
        return failure(&frame.id, "vault_locked");
    }
    if frame.method != "github.liveEvents" && frame.params != json!({}) {
        return failure(&frame.id, "invalid_request");
    }
    let _change = github.changes.lock().await;
    let still_admitted = || {
        state
            .vault
            .as_ref()
            .map(|vault| vault.vault.unlocked_generation())
            == admitted_generation
    };
    if !still_admitted() {
        return failure(&frame.id, "vault_locked");
    }
    let caller = caller(session);
    let initiator=serde_json::to_string(&json!({"organizationId":caller.organization_id,"principalId":caller.principal_id,"credentialId":caller.credential_id})).expect("caller is serializable");
    if frame.method == "github.liveEvents" {
        let Ok(params) = serde_json::from_value::<GithubLiveEventsParams>(frame.params.clone())
        else {
            return failure(&frame.id, "invalid_request");
        };
        let Some(live) = &github.live else {
            return failure(&frame.id, "github_live_unavailable");
        };
        let Some(generation) = admitted_generation.flatten() else {
            return failure(&frame.id, "vault_locked");
        };
        if let Err(error) = live
            .set(
                generation,
                params.enabled,
                &initiator,
                params.accept_gaps.unwrap_or(false),
            )
            .await
        {
            return failure(&frame.id, relay_code(&error));
        }
        if !still_admitted() {
            return failure(&frame.id, "vault_locked");
        }
    }
    if frame.method == "github.disconnect" {
        if let Some(live) = &github.live {
            let Some(generation) = admitted_generation.flatten() else {
                return failure(&frame.id, "vault_locked");
            };
            if let Err(error) = live.set(generation, false, &initiator, false).await {
                return failure(&frame.id, relay_code(&error));
            }
            if !still_admitted() {
                return failure(&frame.id, "vault_locked");
            }
        }
    }
    let answer = match frame.method.as_str() {
        "github.status" | "github.liveEvents" => Ok(github.connections.status()),
        "github.connect" => github.connections.connect(caller).await,
        "github.cancel" => github.connections.cancel(caller).await,
        "github.disconnect" => github.connections.disconnect(caller).await,
        _ => return failure(&frame.id, "unknown_method"),
    };
    match answer {
        Ok(status) => {
            let mut wire = connection(status, &github.install_url);
            if let Some(live) = &github.live {
                match live.status().await {
                    Ok((enabled, repositories, gaps)) => {
                        wire.live_events = Some(GithubLiveEvents {
                            enabled,
                            repositories: repositories as u64,
                            gaps: gaps as u64,
                        })
                    }
                    Err(error) => return failure(&frame.id, relay_code(&error)),
                }
            }
            success(&frame.id, &wire)
        }
        Err(error) => failure(&frame.id, code(error).as_str()),
    }
}

/// What the github methods are given: the sign-in, and where its repositories
/// are chosen.
pub struct GithubRoutes {
    pub connections: std::sync::Arc<GithubConnections>,
    pub install_url: String,
    pub live: Option<std::sync::Arc<crate::github_relay::application::LiveEvents>>,
    pub changes: tokio::sync::Mutex<()>,
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
        live_events: None,
        state: GithubConnectState::SignedOut,
        account: None,
        code: None,
        last: None,
        unrecorded: None,
        install_url: install_url.to_owned(),
    };
    match status {
        GithubStatus::SignedOut { last, unrecorded } => {
            wire.last = last.map(end);
            wire.unrecorded = unrecorded.then_some(true);
        }
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
        GithubStatus::Connected {
            account,
            unrecorded,
        } => {
            wire.unrecorded = unrecorded.then_some(true);
            wire.state = GithubConnectState::Connected;
            wire.account = Some(GithubAccount {
                id: account.id,
                login: account.login,
            });
        }
    }
    wire
}

fn relay_code(error: &crate::github_relay::domain::Failure) -> &'static str {
    use crate::github_relay::domain::Failure;
    match error {
        Failure::Capacity => "github_live_capacity",
        Failure::VaultLocked => "vault_locked",
        _ => "github_live_unavailable",
    }
}
