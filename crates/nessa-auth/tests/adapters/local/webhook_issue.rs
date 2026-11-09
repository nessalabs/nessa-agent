use super::tests::{bootstrap, grant, open_store};
use super::*;
use std::fs;

fn request() -> IssueCredentialRequest {
    IssueCredentialRequest {
        purpose: CredentialIssuePurpose::Webhook,
        request_id: "webhook-issue".into(),
        issuer_principal_id: "owner".into(),
        credential_id: "webhook-credential".into(),
        principal: PrincipalInputDto {
            id: "webhook".into(),
            kind: PrincipalKindDto::Integration,
        },
        membership: MembershipInputDto {
            id: "webhook-membership".into(),
            principal_id: "webhook".into(),
            organization_id: "org-1".into(),
            role: MembershipRoleDto::Member,
            state: MembershipStateDto::Active,
        },
        audience_id: "gateway-1".into(),
        issued_at: 120,
        expires_at: Some(190),
        grants: vec![grant("org-1", "conversation.read")],
    }
}

#[test]
fn webhook_issue_asks_cedar_before_registry_effects() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("auth/credentials.v1.json");
    let store = open_store(&path).unwrap();
    store.bootstrap(bootstrap()).unwrap();
    let before = fs::read(&path).unwrap();
    let mut refused = Vec::new();
    for kind in [
        PrincipalKindDto::Human,
        PrincipalKindDto::Agent,
        PrincipalKindDto::Gateway,
    ] {
        let mut command = request();
        command.principal.kind = kind;
        refused.push(command);
    }
    for action in [
        "conversation.write",
        "credential.manage",
        "server.read",
        "environment.serve",
    ] {
        let mut command = request();
        command.grants.push(grant("org-1", action));
        refused.push(command);
    }
    let mut admin = request();
    admin.membership.role = MembershipRoleDto::Admin;
    refused.push(admin);
    let mut foreign = request();
    foreign.grants[0].resource.id = "other-gateway".into();
    refused.push(foreign);
    for command in refused {
        assert!(matches!(
            store.issue_sync(command),
            Err(LocalStoreError::Conflict)
        ));
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    assert!(matches!(
        store.issue_sync(request()),
        Ok(IssueCredentialOutcome::Issued { .. })
    ));
}

#[test]
fn issuing_purpose_is_bound_to_the_existing_receipt_across_restart() {
    for first_purpose in [
        CredentialIssuePurpose::Product,
        CredentialIssuePurpose::Webhook,
    ] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = open_store(&path).unwrap();
        store.bootstrap(bootstrap()).unwrap();
        let mut first = request();
        first.purpose = first_purpose;
        let IssueCredentialOutcome::Issued {
            metadata,
            transitions,
            ..
        } = store.issue_sync(first.clone()).unwrap()
        else {
            panic!("fresh issue")
        };
        assert_eq!(metadata.id, first.credential_id);
        drop(store);
        let store = open_store(&path).unwrap();
        let before = fs::read(&path).unwrap();
        let mut different = first.clone();
        different.purpose = match first_purpose {
            CredentialIssuePurpose::Product => CredentialIssuePurpose::Webhook,
            CredentialIssuePurpose::Webhook => CredentialIssuePurpose::Product,
        };
        assert!(matches!(
            store.issue_sync(different),
            Err(LocalStoreError::Conflict)
        ));
        first.credential_id = "unused-retry-proposal".into();
        first.issued_at += 1;
        let IssueCredentialOutcome::ExistingSecretUnavailable {
            metadata: replayed,
            transitions: original,
        } = store.issue_sync(first).unwrap()
        else {
            panic!("no secret replay")
        };
        assert_eq!(replayed, metadata);
        assert_eq!(original, transitions);
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}
