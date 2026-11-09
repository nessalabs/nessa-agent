use super::NativeVault;
use crate::{
    gateway_endpoint::application::GatewayEndpointAccess,
    surface_credential::{CredentialRefusal, SurfaceCredentials},
    vault::application::{execute, Command, Failure, VaultConnection},
};
use nessa_gateway_endpoint::{application::EndpointDiscovery, domain::GatewayEndpoint};
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
    crate::test_executable::install(&binary, &script);
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
        crate::test_executable::install(&binary, &script);
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

#[test]
fn a_lost_or_malformed_change_reply_is_uncertain_without_replay() {
    for response in ["", "printf 'malformed\\n'", "printf '{\"ok\":true}'"] {
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("vault-child");
        let starts = root.path().join("starts");
        let calls = root.path().join("calls");
        let quote =
            |path: &std::path::Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
        let script = format!("#!/bin/sh\nIFS= read -r init || exit 1\nprintf x >> {}\nprintf '{{\"ready\":true}}\\n'\nIFS= read -r command || exit 1\nprintf x >> {}\n{}\nexit 0\n", quote(&starts), quote(&calls), response);
        crate::test_executable::install(&binary, &script);
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
        let command = serde_json::from_value(serde_json::json!({"operation":"changePassphrase","passphrase":"disposable replacement phrase"})).unwrap();
        let result = vault.command(command, vault.next_request());
        assert!(
            matches!(result, Err(Failure::VaultChangeNotConfirmed)),
            "{result:?}"
        );
        assert_eq!(std::fs::read(starts).unwrap(), b"x");
        assert_eq!(
            std::fs::read(calls).unwrap(),
            b"x",
            "change must not replay"
        );
        vault.close();
    }
}

#[test]
fn change_publication_and_audit_codes_pass_through_the_native_pipe() {
    for code in [
        "vault_change_unchanged",
        "vault_change_unchanged_audit_unavailable",
        "vault_change_not_confirmed",
        "vault_change_not_confirmed_audit_unavailable",
        "vault_changed_audit_unavailable",
        "vault_forbidden",
    ] {
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("vault-child");
        let script = format!("#!/bin/sh\nIFS= read -r init || exit 1\nprintf '{{\"ready\":true}}\\n'\nIFS= read -r command || exit 1\ncase \"$command\" in *'\"operation\":\"changePassphrase\"'*) ;; *) exit 2 ;; esac\nprintf '{{\"ok\":false,\"code\":\"{}\"}}\\n'\nIFS= read -r end\n", code);
        crate::test_executable::install(&binary, &script);
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
        let command = serde_json::from_value(serde_json::json!({"operation":"changePassphrase","passphrase":"disposable replacement phrase"})).unwrap();
        let result = vault.command(command, vault.next_request()).unwrap();
        assert_eq!(result["code"], code);
        vault.close();
    }
}

#[test]
fn wrong_closed_change_replies_retire_the_child_before_late_answers_can_bleed() {
    for invalid in [
        "null",
        "{\"ok\":true}",
        "{\"ok\":true,\"status\":{\"state\":\"unlocked\"}}",
        "{\"ok\":true,\"status\":{\"state\":\"unrecognized\"}}",
        "{\"code\":\"vault_change_unchanged\"}",
        "{\"ok\":\"false\",\"code\":\"vault_change_unchanged\"}",
        "{\"ok\":false,\"code\":\"unrecognized-code\"}",
        "{\"ok\":true,\"ok\":true,\"status\":{\"state\":\"locked\"}}",
        "{\"ok\":true,\"status\":{\"state\":\"unlocked\",\"state\":\"locked\"}}",
        "{\"ok\":true,\"status\":{\"state\":\"locked\"},\"extra\":true}",
    ] {
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("vault-child");
        let starts = root.path().join("starts");
        let first = root.path().join("first");
        let calls = root.path().join("calls");
        let quote =
            |path: &std::path::Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
        let invalid = invalid.replace('\'', "'\\''");
        let script = format!("#!/bin/sh\nIFS= read -r init || exit 1\nprintf x >> {}\nprintf '{{\"ready\":true}}\\n'\nIFS= read -r command || exit 1\nprintf x >> {}\nif [ ! -e {} ]; then\nprintf x > {}\nprintf '%s\\n' '{}'\nprintf '{{\"ok\":true,\"status\":{{\"state\":\"unlocked\"}}}}\\n'\nIFS= read -r late || exit 0\nelse\nprintf '{{\"ok\":true,\"status\":{{\"state\":\"locked\"}}}}\\n'\nIFS= read -r end\nfi\n",quote(&starts),quote(&calls),quote(&first),quote(&first),invalid);
        crate::test_executable::install(&binary, &script);
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
        let change = serde_json::from_value(serde_json::json!({"operation":"changePassphrase","passphrase":"disposable replacement phrase"})).unwrap();
        assert!(
            matches!(
                vault.command(change, vault.next_request()),
                Err(Failure::VaultChangeNotConfirmed)
            ),
            "wrong-shaped change answer must remain uncertain"
        );
        let status = vault.command(request(), vault.next_request()).unwrap();
        assert_eq!(
            status["status"]["state"], "locked",
            "late old reply must not satisfy the explicit new status request"
        );
        assert_eq!(
            std::fs::read(starts).unwrap(),
            b"xx",
            "malformed child must be retired"
        );
        assert_eq!(
            std::fs::read(calls).unwrap(),
            b"xx",
            "only one change and one explicit status are sent"
        );
        vault.close();
    }
}
