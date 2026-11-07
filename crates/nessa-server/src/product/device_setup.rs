//! `devices.sshHosts`, `devices.connectSsh` and `devices.setDefault`: the
//! wire's words for [`DeviceSetup`]. The socket has already asked Cedar for
//! `credential.manage`; what may be connected or chosen is the setup's.
//! `devices.list` names the configured hosts and the default through
//! [`listed`].
use super::{
    pairing::linking_caller,
    socket::{failure, success},
    state::ProductRouteState,
};
use crate::devices::application::{
    Connect, ConnectRefusal, ConnectState, DefaultRefusal, DeviceSetup, SshHost,
};
use nessa_auth::application::session::AuthenticatedSession;
use nessa_protocol::product::generated::{
    DeviceSshConnect, DeviceSshHost, DevicesConnectSshParams, DevicesConnectSshResult,
    DevicesListResult, DevicesSetDefaultParams, DevicesSetDefaultResult, DevicesSshHostsResult,
};
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};
use nessa_sdk::domain::agent_execution::leases::{LeaseRefusal, SshDestination};
use serde_json::json;
use std::sync::Arc;

const UNAVAILABLE: &str = "devices_unavailable";

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    let Some(setup) = state.device_setup.clone() else {
        return failure(&frame.id, UNAVAILABLE);
    };
    match frame.method.as_str() {
        "devices.sshHosts" => {
            if frame.params != json!({}) {
                return failure(&frame.id, "invalid_request");
            }
            let Ok((suggested, connecting)) =
                tokio::task::spawn_blocking(move || setup.ssh_hosts()).await
            else {
                return failure(&frame.id, UNAVAILABLE);
            };
            success(
                &frame.id,
                &DevicesSshHostsResult {
                    suggested: suggested.into_iter().map(wire_host).collect(),
                    connecting: connecting.into_iter().map(wire_connect).collect(),
                },
            )
        }
        "devices.connectSsh" => {
            let Ok(params) = serde_json::from_value::<DevicesConnectSshParams>(frame.params) else {
                return failure(&frame.id, "invalid_request");
            };
            let Ok(host) = SshDestination::new(params.host) else {
                return failure(&frame.id, "invalid_request");
            };
            let state = setup.connect(linking_caller(session), host.clone());
            success(
                &frame.id,
                &DevicesConnectSshResult {
                    connecting: wire_connect(Connect { host, state }),
                },
            )
        }
        "devices.setDefault" => {
            let Ok(params) = serde_json::from_value::<DevicesSetDefaultParams>(frame.params) else {
                return failure(&frame.id, "invalid_request");
            };
            match setup
                .set_default(linking_caller(session), params.environment)
                .await
            {
                Ok(default_environment) => success(
                    &frame.id,
                    &DevicesSetDefaultResult {
                        default_environment,
                    },
                ),
                Err(DefaultRefusal::NotADevice) => failure(&frame.id, "devices_not_a_device"),
                Err(DefaultRefusal::Busy) => failure(&frame.id, "devices_busy"),
                Err(DefaultRefusal::Unavailable) => failure(&frame.id, UNAVAILABLE),
            }
        }
        _ => failure(&frame.id, "method_not_found"),
    }
}

/// `list`, the first page of `devices.list`, with the SSH hosts this run
/// serves and the saved default device.
pub(super) async fn listed(
    setup: Option<&Arc<DeviceSetup>>,
    mut list: DevicesListResult,
) -> DevicesListResult {
    let Some(setup) = setup.cloned() else {
        return list;
    };
    list.default_environment = setup.default_environment();
    let hosts = tokio::task::spawn_blocking(move || setup.configured())
        .await
        .unwrap_or_default();
    list.ssh_hosts = Some(hosts.into_iter().map(wire_host).collect());
    list
}

fn wire_host(host: SshHost) -> DeviceSshHost {
    DeviceSshHost {
        host: host.host.as_str().to_owned(),
        host_name: host.host_name,
        user: host.user,
        port: host.port.map(u64::from),
    }
}

/// An install's refusal in a lease's words. An install answers only these
/// four; any other is reported as the install failing.
fn install_refusal(refusal: LeaseRefusal) -> &'static str {
    match refusal {
        LeaseRefusal::EnvironmentUnreachable => "environment_unreachable",
        LeaseRefusal::EnvironmentPlatformUnsupported => "environment_platform_unsupported",
        LeaseRefusal::EnvironmentVersionMismatch => "environment_version_mismatch",
        _ => "environment_install_failed",
    }
}

fn wire_connect(connect: Connect) -> DeviceSshConnect {
    let (state, refusal) = match connect.state {
        ConnectState::Installing => ("installing", None),
        ConnectState::Restarting => ("restarting", None),
        ConnectState::Connected => ("connected", None),
        ConnectState::Failed(refusal) => (
            "failed",
            Some(match refusal {
                ConnectRefusal::Install(refusal) => install_refusal(refusal),
                ConnectRefusal::HostLimit => "devices_host_limit",
                ConnectRefusal::Busy => "devices_busy",
                ConnectRefusal::Unavailable => UNAVAILABLE,
            }),
        ),
    };
    DeviceSshConnect {
        host: connect.host.as_str().to_owned(),
        state: state.to_owned(),
        refusal: refusal.map(str::to_owned),
    }
}
