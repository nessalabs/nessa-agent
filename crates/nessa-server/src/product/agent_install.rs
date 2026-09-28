//! Authenticated native-runtime downloads. A blocking worker owns the single
//! install permit through durable publication and audit, even if its waiter leaves.
use super::{
    generated::{
        AgentInstallOffer, AgentInstallOptionsResult, AgentInstallParams, AgentInstallResult,
    },
    socket::{failure, success},
    state::ProductRouteState,
};
use crate::{
    agent_install::{
        application::{GatewayInstallFailure, InstallFailure, SourceFailure, StoreFailure},
        domain::{AgentName, InstallRequest},
    },
    protocol::{OutgoingMessage, RequestFrame},
};
use nessa_auth::application::session::AuthenticatedSession;

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    let Some(installer) = state.agent_installations.clone() else {
        return failure(&frame.id, "agent_installer_unavailable");
    };
    if frame.method == "agents.installOptions" {
        if frame.params != serde_json::json!({}) {
            return failure(&frame.id, "invalid_request");
        }
        return match tokio::task::spawn_blocking(move || installer.offers()).await {
            Ok(Ok(offers)) => {
                let agents = offers
                    .into_iter()
                    .map(|offer| {
                        Ok(AgentInstallOffer {
                            agent: serde_json::from_value(serde_json::json!(offer.agent.as_str()))?,
                            version: offer.version,
                            archive_bytes: offer.archive_bytes,
                            installed: offer.installed,
                        })
                    })
                    .collect::<Result<Vec<_>, serde_json::Error>>();
                match agents {
                    Ok(agents) => success(&frame.id, &AgentInstallOptionsResult { agents }),
                    Err(_) => failure(&frame.id, "agent_installer_unavailable"),
                }
            }
            _ => failure(&frame.id, "agent_installer_unavailable"),
        };
    }
    let Ok(params) = serde_json::from_value::<AgentInstallParams>(frame.params) else {
        return failure(&frame.id, "invalid_request");
    };
    let Ok(agent) = AgentName::parse(params.agent.as_str()) else {
        return failure(&frame.id, "invalid_request");
    };
    let context = session.context();
    let invocation = serde_json::json!([
        "gateway",
        agent.as_str(),
        context.organization_id().as_str(),
        context.principal_id().as_str(),
        context.credential_id().as_str(),
        params.request_id
    ])
    .to_string();
    let Ok(request) = InstallRequest::new(installer.account_id(), invocation) else {
        return failure(&frame.id, "invalid_request");
    };
    let Ok(permit) = state.installs.clone().try_acquire_owned() else {
        return failure(&frame.id, "agent_install_busy");
    };
    let name = params.agent;
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        installer.install(&agent, &request)
    })
    .await
    {
        Ok(Ok(installed)) => success(
            &frame.id,
            &AgentInstallResult {
                agent: name,
                version: installed.version.as_str().into(),
                downloaded: installed.downloaded,
                cleanup_pending: !installed.reclamation_warnings.is_empty(),
            },
        ),
        Ok(Err(error)) => failure(&frame.id, code(&error)),
        Err(_) => failure(&frame.id, "agent_install_not_confirmed"),
    }
}

fn code(error: &GatewayInstallFailure) -> &'static str {
    match error {
        GatewayInstallFailure::Unavailable => "agent_installer_unavailable",
        GatewayInstallFailure::Unsupported => "agent_install_unsupported",
        GatewayInstallFailure::Install(error) => match error.as_ref() {
            InstallFailure::UnsupportedPlatform(_) => "agent_install_unsupported",
            InstallFailure::Download(SourceFailure::TooLarge(_))
            | InstallFailure::Rejected(_)
            | InstallFailure::Store(
                StoreFailure::IncompleteArchive(_) | StoreFailure::MalformedArchive(_),
            ) => "agent_install_verification_failed",
            InstallFailure::Download(SourceFailure::NotStored(_))
            | InstallFailure::Store(StoreFailure::Unwritable(_) | StoreFailure::Unreadable(_)) => {
                "agent_install_storage_failed"
            }
            InstallFailure::Download(_) => "agent_download_failed",
            _ => "agent_install_not_confirmed",
        },
    }
}
