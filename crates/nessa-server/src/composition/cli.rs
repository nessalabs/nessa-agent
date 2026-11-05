//! Construct the CLI's local gateway adapter and route command output.
use super::local_auth::SystemClock;
use crate::{
    cli::{
        application::{invoke, issue_token, refusal_body, Gateway},
        infrastructure::{serve_mcp, LocalGateway},
    },
    core::{local_timings, RunError},
    env::Environment,
};
use nessa_auth::application::ports::Clock;
use nessa_client_core::local_product::ProductCaller;
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

/// Send one catalog method and print the handler's JSON.
pub(super) fn call(
    method: &str,
    params: &str,
    credential_file: Option<PathBuf>,
) -> Result<(), RunError> {
    let params = serde_json::from_str(params)
        .map_err(|_| failure("call --params must be one JSON value"))?;
    let mut gateway = connect(credential_file, ProductCaller::CLI)?;
    finish(invoke(&mut gateway, method, params))
}

/// Serve the catalog as MCP tools on stdin and stdout, as the nessa-mcp credential.
pub(super) fn mcp(credential_file: Option<PathBuf>) -> Result<(), RunError> {
    let mut gateway = connect(credential_file, ProductCaller::MCP)?;
    serve_mcp(&mut gateway, std::io::stdin().lock(), std::io::stdout())
        .map_err(|error| failure(error.to_string()))
}

fn finish(
    outcome: Result<serde_json::Value, crate::cli::application::CallFailure>,
) -> Result<(), RunError> {
    match outcome {
        Ok(payload) => {
            serde_json::to_writer(io::stdout().lock(), &payload)
                .and_then(|()| writeln!(io::stdout()).map_err(serde_json::Error::io))
                .map_err(|_| failure("call output failed"))?;
            Ok(())
        }
        Err(error) => {
            serde_json::to_writer(io::stdout().lock(), &refusal_body(&error))
                .and_then(|()| writeln!(io::stdout()).map_err(serde_json::Error::io))
                .map_err(|_| failure("call output failed"))?;
            Err(failure(error.to_string()))
        }
    }
}

pub(super) fn online(
    token: bool,
    credential_file: Option<PathBuf>,
    ttl_seconds: Option<u64>,
) -> Result<(), RunError> {
    let config = Environment::from_system()?;
    let timings = (!token).then(|| {
        local_timings::doctor_timings(
            Environment::log_directory_from_system()
                .ok()
                .flatten()
                .as_deref(),
        )
    });
    let address = match gateway_address(&config) {
        Ok(address) => address,
        Err(error) => {
            return if let Some(timings) = timings {
                finish_doctor(json!({
                    "ok": false,
                    "backend": "local",
                    "authenticated": false,
                    "healthy": false,
                    "error": error.to_string(),
                    "timings": timings,
                }))
            } else {
                Err(error)
            };
        }
    };
    let file = match credential_file
        .or_else(|| {
            config
                .auth_directory
                .as_ref()
                .map(|root| root.join("surfaces/nessa-cli.token"))
        })
        .ok_or_else(|| failure("local credential directory unavailable"))
    {
        Ok(file) => file,
        Err(error) => {
            return if let Some(timings) = timings {
                finish_doctor(json!({
                    "ok": false,
                    "backend": "local",
                    "authenticated": false,
                    "healthy": false,
                    "error": error.to_string(),
                    "timings": timings,
                }))
            } else {
                Err(error)
            };
        }
    };
    let connected = LocalGateway::connect(address, &file);
    if !token {
        let mut report = match connected {
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
        if let Some(object) = report.as_object_mut() {
            object.insert("timings".to_owned(), timings.unwrap_or_else(|| json!({})));
        }
        return finish_doctor(report);
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

fn connect(
    credential_file: Option<PathBuf>,
    caller: ProductCaller,
) -> Result<LocalGateway, RunError> {
    let config = Environment::from_system()?;
    let address = gateway_address(&config)?;
    let file = credential_file
        .or_else(|| {
            config
                .auth_directory
                .as_ref()
                .map(|root| root.join(format!("surfaces/{}.token", caller.instance)))
        })
        .ok_or_else(|| failure("local credential directory unavailable"))?;
    LocalGateway::connect_as(address, &file, caller).map_err(|error| failure(error.to_string()))
}

fn gateway_address(config: &Environment) -> Result<SocketAddr, RunError> {
    let address: SocketAddr = config
        .listen_addr()
        .parse()
        .map_err(|_| failure("invalid local gateway address"))?;
    match config.gateway_endpoint_storage() {
        Some((root, directory)) => Ok(DiscoverGatewayEndpoint::new(&FileEndpointDiscovery::new(
            root, directory,
        ))
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
        .unwrap_or(address)),
        None => Ok(address),
    }
}

fn finish_doctor(report: serde_json::Value) -> Result<(), RunError> {
    serde_json::to_writer(io::stdout().lock(), &report)
        .map_err(|_| failure("doctor output failed"))?;
    writeln!(io::stdout()).map_err(|_| failure("doctor output failed"))?;
    if report["ok"] == true {
        Ok(())
    } else {
        Err(failure("doctor found a local gateway problem"))
    }
}

fn failure(message: impl Into<String>) -> RunError {
    RunError::Authentication(message.into())
}
