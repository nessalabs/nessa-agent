use super::NativeVault;
use crate::{
    gateway_endpoint::application::GatewayEndpointAccess,
    surface_credential::{CredentialRefusal, SurfaceCredentials},
    vault::application::{execute, Command, Failure, VaultConnection},
};
use nessa_gateway_endpoint::{application::EndpointDiscovery, domain::GatewayEndpoint};
use std::os::unix::fs::PermissionsExt;
use std::{io, sync::Arc, time::Duration};
struct NoPublication;
impl EndpointDiscovery for NoPublication {
    fn discover(&self) -> io::Result<Option<GatewayEndpoint>> {
        Ok(None)
    }
}
struct Credential;
impl SurfaceCredentials for Credential {
    fn read(&self, _: &str) -> Result<String, CredentialRefusal> {
        Ok("disposable-fixture-credential".into())
    }
}
fn request() -> Command {
    serde_json::from_value(serde_json::json!({"operation":"status"})).unwrap()
}
#[test]
fn timeout_reaps_a_stalled_child_and_a_stale_timeout_cannot_kill_its_replacement() {
    let root = tempfile::tempdir().unwrap();
    let binary = root.path().join("vault-child");
    let marker = root.path().join("first-command");
    let starts = root.path().join("starts");
    let quote =
        |path: &std::path::Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
    let script=format!("#!/bin/sh\nIFS= read -r init || exit 1\nprintf x >> {}\nprintf '{{\"ready\":true}}\\n'\nwhile IFS= read -r command; do\nif [ ! -e {} ]; then\nprintf x > {}\nIFS= read -r stalled || exit 0\nelse\nprintf '{{\"ok\":true,\"status\":{{\"state\":\"locked\"}}}}\\n'\nfi\ndone\n",quote(&starts),quote(&marker),quote(&marker));
    std::fs::write(&binary, script).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let vault = Arc::new(NativeVault::new(
        binary,
        Arc::new(GatewayEndpointAccess::new(
            "dev".into(),
            Arc::new(NoPublication),
        )),
        Arc::new(Credential),
        "dev".into(),
        9137,
    ));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    runtime.block_on(async {
        let result = execute(vault.clone(), request(), Duration::from_secs(2)).await;
        assert!(matches!(result, Err(Failure::VaultUnavailable)));
        assert!(
            marker.exists(),
            "child must reach the stalled command; starts={}, result={result:?}",
            starts.exists()
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match execute(vault.clone(), request(), Duration::from_secs(2)).await {
                Ok(value) => {
                    assert_eq!(value["status"]["state"], "locked");
                    break;
                }
                Err(Failure::VaultBusy) => {
                    assert!(std::time::Instant::now() < deadline);
                    tokio::task::yield_now().await;
                }
                Err(other) => panic!("replacement request failed: {other:?}"),
            }
        }
        vault.interrupt(1);
        assert_eq!(
            execute(vault.clone(), request(), Duration::from_secs(2))
                .await
                .unwrap()["status"]["state"],
            "locked"
        );
        assert_eq!(
            std::fs::read(&starts).unwrap().len(),
            2,
            "stale timeout must not restart the replacement"
        );
        vault.close();
        assert!(matches!(
            execute(vault.clone(), request(), Duration::from_secs(2)).await,
            Err(Failure::VaultUnavailable)
        ));
    });
}

#[test]
fn only_status_reconnects_once_after_a_redacted_transport_failure() {
    for readonly in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("vault-child");
        let starts = root.path().join("starts");
        let marker = root.path().join("first");
        let quote =
            |path: &std::path::Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
        let script=format!("#!/bin/sh\nIFS= read -r init || exit 1\nprintf x >> {}\nprintf '{{\"ready\":true}}\\n'\nIFS= read -r command || exit 1\nif [ ! -e {} ]; then\nprintf x > {}\nprintf '{{\"ok\":false,\"code\":\"vault_unavailable\"}}\\n'\nelse\nprintf '{{\"ok\":true,\"status\":{{\"state\":\"locked\"}}}}\\n'\nfi\nIFS= read -r end\n",quote(&starts),quote(&marker),quote(&marker));
        std::fs::write(&binary, script).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let vault = NativeVault::new(
            binary,
            Arc::new(GatewayEndpointAccess::new(
                "dev".into(),
                Arc::new(NoPublication),
            )),
            Arc::new(Credential),
            "dev".into(),
            9137,
        );
        let command = serde_json::from_value(if readonly {
            serde_json::json!({"operation":"status"})
        } else {
            serde_json::json!({"operation":"unlock","passphrase":"disposable long passphrase"})
        })
        .unwrap();
        let result = vault.command(command, vault.next_request()).unwrap();
        if readonly {
            assert_eq!(result["status"]["state"], "locked");
            assert_eq!(std::fs::read(&starts).unwrap().len(), 2);
        } else {
            assert_eq!(result["ok"], false);
            assert_eq!(
                std::fs::read(&starts).unwrap().len(),
                1,
                "an uncertain unlock is not replayed"
            );
        }
        vault.close();
    }
}
