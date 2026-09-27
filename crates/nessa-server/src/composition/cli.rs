//! Construct the CLI's local gateway adapter and route command output.
use super::local_auth::SystemClock;
use crate::{
    cli::{
        application::{issue_token, Gateway},
        infrastructure::LocalGateway,
    },
    core::RunError,
    env::Environment,
};
use nessa_auth::application::ports::Clock;
use nessa_gateway_endpoint::{
    application::DiscoverGatewayEndpoint, infrastructure::FileEndpointDiscovery,
};
use serde_json::json;
use std::{
    io::{self, Write},
    net::SocketAddr,
    path::PathBuf,
};
use uuid::Uuid;

pub(super) fn online(
    token: bool,
    credential_file: Option<PathBuf>,
    ttl_seconds: Option<u64>,
) -> Result<(), RunError> {
    let config = Environment::from_system()?;
    let address: SocketAddr = config
        .listen_addr()
        .parse()
        .map_err(|_| failure("invalid local gateway address"))?;
    let address = match config.gateway_endpoint_storage() {
        Some((root, directory)) => {
            DiscoverGatewayEndpoint::new(&FileEndpointDiscovery::new(root, directory))
                .execute()
                .map_err(|error| failure(error.to_string()))?
                .map(|endpoint| {
                    endpoint
                        .web_socket_url()
                        .trim_start_matches("ws://")
                        .parse()
                        .map_err(|_| failure("invalid published gateway address"))
                })
                .transpose()?
                .unwrap_or(address)
        }
        None => address,
    };
    let file = credential_file
        .or_else(|| {
            config
                .auth_directory
                .as_ref()
                .map(|root| root.join("surfaces/nessa-cli.token"))
        })
        .ok_or_else(|| failure("local credential directory unavailable"))?;
    let connected = LocalGateway::connect(address, &file);
    if !token {
        let report = match connected {
            Ok(mut gateway) => match gateway.health() {
                Ok(()) => {
                    json!({"ok":true,"backend":"local","stage":config.stage.as_str(),"gateway":address.to_string(),"authenticated":true,"healthy":true,"gatewayId":gateway.identity().gateway_id,"principalId":gateway.identity().principal_id})
                }
                Err(error) => {
                    json!({"ok":false,"backend":"local","authenticated":true,"healthy":false,"error":error.to_string()})
                }
            },
            Err(error) => {
                json!({"ok":false,"backend":"local","authenticated":false,"healthy":false,"error":error.to_string()})
            }
        };
        serde_json::to_writer(io::stdout().lock(), &report)
            .map_err(|_| failure("doctor output failed"))?;
        writeln!(io::stdout()).map_err(|_| failure("doctor output failed"))?;
        return if report["ok"] == true {
            Ok(())
        } else {
            Err(failure("doctor found a local gateway problem"))
        };
    }
    let mut gateway = connected.map_err(|error| failure(error.to_string()))?;
    let request_id = Uuid::new_v4().to_string();
    tracing::info!(%request_id, "requesting browser credential; issuance is never automatically retried");
    let result = issue_token(&mut gateway, request_id.clone(), Uuid::new_v4().to_string(), Uuid::new_v4().to_string(), SystemClock.unix_seconds(), ttl_seconds)
        .map_err(|error| failure(format!("{error}; requestId={request_id}; if delivery was interrupted, inspect gateway credentials before issuing again")))?;
    tracing::info!(credential_id = %result.credential_id, %request_id, "browser credential issued");
    writeln!(io::stdout().lock(), "{}", result.secret).map_err(|_| {
        failure(format!(
            "credential {} was issued but token output failed; revoke it before issuing again",
            result.credential_id
        ))
    })?;
    Ok(())
}
fn failure(message: impl Into<String>) -> RunError {
    RunError::Authentication(message.into())
}
