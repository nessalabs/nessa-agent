//! Credential-managed Boat setup. Ordinary reads never expose the saved token.
#[cfg(unix)]
use super::socket::success;
use super::{socket::failure, state::ProductRouteState};
#[cfg(unix)]
use crate::{
    conversation::{
        application::ConversationError,
        domain::{ChannelId, VmLifetime, VmLifetimeScope},
    },
    workspaces::{
        application::{RuntimeCaller, Workspaces},
        domain::Failure,
    },
};
use nessa_auth::application::session::AuthenticatedSession;
#[cfg(unix)]
use nessa_protocol::product::generated::{
    BoatRuntimeReadParams, BoatRuntimeSelection, BoatRuntimeSetParams, BoatRuntimeSettings,
    BoatSaveParams, BoatVmLifetime, VmLifetimeMode, NON_NULL_BOAT_RUNTIME_OPTIONS,
};
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    #[cfg(not(unix))]
    {
        let _ = (state, session);
        failure(&frame.id, "boat_not_supported")
    }
    #[cfg(unix)]
    {
        let mut frame = frame;
        let Some(service) = &state.workspaces else {
            return failure(&frame.id, "boat_unavailable");
        };
        if matches!(
            frame.method.as_str(),
            "boat.runtimeSettings" | "boat.setRuntimeSettings"
        ) {
            return runtime_dispatch(state, service, session, frame).await;
        }
        let result = match frame.method.as_str() {
            "boat.status" if frame.params.is_null() || frame.params == serde_json::json!({}) => {
                Ok(())
            }
            "boat.disconnect"
                if frame.params.is_null() || frame.params == serde_json::json!({}) =>
            {
                service.settings.clear().await
            }
            "boat.save" => {
                let Ok(params) = serde_json::from_value::<BoatSaveParams>(std::mem::replace(
                    &mut frame.params,
                    serde_json::json!({}),
                )) else {
                    return failure(&frame.id, "invalid_request");
                };
                service.save_token(params.token).await
            }
            _ => return failure(&frame.id, "invalid_request"),
        };
        if let Err(error) = result {
            return failure(&frame.id, code(error));
        }
        match service.settings.read().await {
            Ok(saved) => success(
                &frame.id,
                &serde_json::json!({"state":"connected","environment":saved.environment,"image":saved.image}),
            ),
            Err(Failure::Locked) => success(&frame.id, &serde_json::json!({"state":"locked"})),
            Err(Failure::NotConfigured) => {
                success(&frame.id, &serde_json::json!({"state":"disconnected"}))
            }
            Err(error) => failure(&frame.id, code(error)),
        }
    }
}
#[cfg(unix)]
fn code(error: Failure) -> &'static str {
    match error {
        Failure::Locked => "vault_locked",
        Failure::NotConfigured => "boat_not_configured",
        Failure::InvalidInput => "invalid_request",
        Failure::Storage => "boat_storage_unavailable",
        Failure::Missing => "boat_resource_missing",
        Failure::Provider => "boat_unreachable",
        Failure::WorkerUnavailable => "boat_worker_unavailable",
        Failure::UnverifiedWorkspace { .. } => "boat_workspace_unverified",
        Failure::RefusedWorkspace { .. } => "boat_admission_refused",
        Failure::AdmissionRejected(_) => "boat_admission_refused",
        Failure::AdmissionRecovery { .. } => "boat_admission_recovery_required",
        Failure::EnvironmentUnavailable => "boat_environment_blocks_credentials",
        Failure::ProvisioningUncertain => "boat_provisioning_uncertain",
        Failure::ReceiptRecovery { .. } | Failure::CleanupUncertain => "boat_cleanup_uncertain",
    }
}

#[cfg(unix)]
async fn runtime_dispatch(
    state: &ProductRouteState,
    service: &Workspaces,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    if NON_NULL_BOAT_RUNTIME_OPTIONS.iter().any(|field| {
        frame
            .params
            .get(*field)
            .is_some_and(serde_json::Value::is_null)
    }) {
        return failure(&frame.id, "invalid_request");
    }
    let (channel, selection) = if frame.method == "boat.setRuntimeSettings" {
        let Ok(params) = serde_json::from_value::<BoatRuntimeSetParams>(frame.params) else {
            return failure(&frame.id, "invalid_request");
        };
        let choice = match (params.mode, params.seconds) {
            (BoatRuntimeSelection::Default, None) => None,
            (BoatRuntimeSelection::Never, None) => Some(VmLifetime::Unlimited),
            (BoatRuntimeSelection::After, Some(seconds)) => {
                let Some(lifetime) = VmLifetime::from_seconds(Some(seconds)) else {
                    return failure(&frame.id, "invalid_request");
                };
                Some(lifetime)
            }
            _ => return failure(&frame.id, "invalid_request"),
        };
        (params.channel_id, Some(choice))
    } else {
        let Ok(params) = serde_json::from_value::<BoatRuntimeReadParams>(frame.params) else {
            return failure(&frame.id, "invalid_request");
        };
        (params.channel_id, None)
    };
    let scope = match channel {
        None => VmLifetimeScope::Global,
        Some(channel) => {
            let Ok(channel) = ChannelId::new(&channel) else {
                return failure(&frame.id, "invalid_request");
            };
            let Some(conversations) = &state.conversations else {
                return failure(&frame.id, "boat_unavailable");
            };
            let caller = super::conversation::caller(session, frame.id.clone());
            if let Err(error) = conversations
                .authorize_channel_settings(&caller, &channel)
                .await
            {
                return failure(
                    &frame.id,
                    match error {
                        ConversationError::Unavailable => "boat_storage_unavailable",
                        _ => "boat_resource_missing",
                    },
                );
            }
            VmLifetimeScope::Channel(channel)
        }
    };
    let result = match selection {
        None => service.runtime_settings.read(&scope).await,
        Some(choice) => {
            let context = session.context();
            service
                .runtime_settings
                .save(
                    scope.clone(),
                    choice,
                    RuntimeCaller {
                        organization: context.organization_id().clone(),
                        principal: context.principal_id().clone(),
                        credential: context.credential_id().as_str().to_owned(),
                        operation: frame.id.clone(),
                    },
                )
                .await
        }
    };
    match result {
        Ok(settings) if settings.scope() != &scope => {
            failure(&frame.id, "boat_storage_unavailable")
        }
        Ok(settings) => success(
            &frame.id,
            &BoatRuntimeSettings {
                channel_id: settings
                    .scope()
                    .channel()
                    .map(|channel| channel.as_str().to_owned()),
                default_lifetime: wire_lifetime(settings.default_lifetime()),
                channel_override: settings.channel_override().map(wire_lifetime),
            },
        ),
        Err(error) => failure(&frame.id, code(error)),
    }
}
#[cfg(unix)]
fn wire_lifetime(lifetime: VmLifetime) -> BoatVmLifetime {
    BoatVmLifetime {
        mode: if lifetime.seconds().is_some() {
            VmLifetimeMode::After
        } else {
            VmLifetimeMode::Never
        },
        seconds: lifetime.seconds().map(u64::from),
    }
}
