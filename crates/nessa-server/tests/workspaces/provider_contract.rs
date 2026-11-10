//! Sanitized provider responses and the published release worker retain the
//! workspace allocation, protocol and pinned host identities we consume.
use boat_client::{SandboxInfoResponse, SandboxState, SshKeyResponse};

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

#[test]
fn a_thread_folder_stays_where_existing_vms_have_it() {
    use crate::workspaces::{domain::Failure, infrastructure::thread_folder};
    // workspace-sdk made `<root>/<thread>/project`; VMs made then still hold it.
    assert_eq!(
        thread_folder("thread-1").unwrap(),
        "/home/user/.nessa/workspaces/thread-1/project"
    );
    // The id goes into a shell command and a path: one plain segment only.
    for refused in ["", ".", "..", "a/b", "a'b", "a b", "$(x)", &"x".repeat(129)] {
        assert!(
            matches!(thread_folder(refused), Err(Failure::InvalidInput)),
            "{refused:?} was accepted"
        );
    }
}
