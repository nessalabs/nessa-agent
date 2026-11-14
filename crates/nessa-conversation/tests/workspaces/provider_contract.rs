//! Sanitized provider responses and the published release worker retain the
//! workspace allocation, protocol and pinned host identities we consume.
use workspace_sdk::boat_client::{SandboxInfoResponse, SandboxState, SshKeyResponse};

#[test]
fn boat_v1_receipts_keep_workspace_and_ssh_trust_identity() {
    let vm: SandboxInfoResponse =
        serde_json::from_str(include_str!("fixtures/boat-v1-sandbox.json")).unwrap();
    assert_eq!(vm.sandbox.id, "bx_fixture");
    assert_eq!(vm.sandbox.state, SandboxState::Idle);
    assert_eq!(vm.sandbox.memory_gb, Some(4));
    assert_eq!(vm.sandbox.vcpu, Some(2));
    assert_eq!(vm.sandbox.environment.as_deref(), Some("nessa"));
    let ssh: SshKeyResponse =
        serde_json::from_str(include_str!("fixtures/boat-v1-ssh-key.json")).unwrap();
    assert_eq!(ssh.success, Some(true));
    assert_eq!(ssh.ssh_user.as_deref(), Some("user"));
    assert!(ssh
        .ssh_endpoint
        .unwrap()
        .parse::<std::net::SocketAddr>()
        .is_ok());
    assert!(ssh.host_key.unwrap().starts_with("ssh-ed25519 "));
}

#[test]
fn shipping_worker_covers_the_compiled_lease_protocol() {
    assert!(
        crate::workspaces::infrastructure::worker_image().is_some(),
        "publish and pin the credential-free worker for this lease protocol before shipping"
    );
}
