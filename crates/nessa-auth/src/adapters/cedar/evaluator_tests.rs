use super::*;
use crate::application::ports::IssuePath;
use crate::domain::{
    AudienceId, Credential, CredentialId, Grant, Membership, MembershipId, MembershipRole,
    MembershipStatus, OrganizationId, PrincipalId, PrincipalKind, ResourceId,
};

fn request_values() -> (AuthContext, Action, Resource, AccessSnapshot) {
    let principal_id = PrincipalId::new("principal-1").unwrap();
    let organization_id = OrganizationId::new("organization-1").unwrap();
    let membership_id = MembershipId::new("membership-1").unwrap();
    let credential_id = CredentialId::new("credential-1").unwrap();
    let audience_id = AudienceId::new("gateway-1").unwrap();
    let action = Action::new("server.read").unwrap();
    let resource = Resource::new(
        organization_id.clone(),
        ResourceId::new("gateway-1").unwrap(),
    );
    let credential = Credential::new(
        credential_id.clone(),
        principal_id.clone(),
        organization_id.clone(),
        audience_id.clone(),
        100,
        200,
        vec![Grant::new(action.clone(), resource.clone())],
    )
    .unwrap();
    let membership = Membership::new(
        membership_id.clone(),
        principal_id.clone(),
        organization_id.clone(),
        MembershipRole::Member,
        MembershipStatus::Active,
    );
    let context = AuthContext::new(
        principal_id,
        organization_id,
        membership_id,
        credential_id,
        audience_id,
    );
    (context, action, resource, {
        let revision = 1;
        // This fixture explicitly creates its own original product issuance.
        let original = crate::domain::CredentialTransition::new(
            credential.id().clone(),
            None,
            crate::domain::CredentialLifecycle {
                issued_at: credential.issued_at(),
                expires_at: credential.expires_at(),
                revoked_at: None,
            },
            crate::domain::TransitionCause::Issued(crate::domain::IssuanceCause::AdminIssue),
            crate::domain::Initiator::LocalOperator,
            credential.issued_at(),
        )
        .unwrap();
        let origin = crate::application::origin::CredentialOrigin::restore(
            crate::application::dto::CredentialTransitionDto::record(&original, 1, 1, None),
            &credential,
        )
        .unwrap();
        AccessSnapshot {
            credential,
            membership,
            revision,
            origin,
        }
    })
}

#[test]
fn checked_in_bundle_is_valid() {
    CedarPolicyEvaluator::new().expect("checked-in Cedar bundle must validate");
}

#[test]
fn read_grant_allows_only_read_for_its_exact_resource() {
    let (context, _, resource, mut snapshot) = request_values();
    let read = Action::new("conversation.read").unwrap();
    let write = Action::new("conversation.write").unwrap();
    snapshot.credential = Credential::new(
        context.credential_id().clone(),
        context.principal_id().clone(),
        context.organization_id().clone(),
        context.audience_id().clone(),
        100,
        200,
        vec![Grant::new(read.clone(), resource.clone())],
    )
    .unwrap();
    let evaluator = CedarPolicyEvaluator::new().unwrap();
    assert_eq!(
        evaluator.evaluate(&context, &read, &resource, &snapshot),
        Ok(Decision::Allow)
    );
    assert_eq!(
        evaluator.evaluate(&context, &write, &resource, &snapshot),
        Ok(Decision::Deny)
    );
    let other = Resource::new(
        context.organization_id().clone(),
        ResourceId::new("another-gateway").unwrap(),
    );
    assert_eq!(
        evaluator.evaluate(&context, &read, &other, &snapshot),
        Ok(Decision::Deny)
    );
}

#[test]
fn malformed_schema_is_rejected() {
    assert!(CedarPolicyEvaluator::from_sources("not json", POLICIES).is_err());
}

#[test]
fn malformed_or_invalid_policy_is_rejected() {
    assert!(CedarPolicyEvaluator::from_sources(SCHEMA, "permit(").is_err());
    assert!(CedarPolicyEvaluator::from_sources(
        SCHEMA,
        r#"@id("unknown") permit(principal, action == Nessa::Action::"unknown", resource);"#,
    )
    .is_err());
}

/// Every decision names the policies that made it, so a policy without an
/// `@id`, or with an empty one, refuses the whole bundle.
#[test]
fn a_policy_without_an_id_is_rejected() {
    let permit = r#"permit(
            principal is Nessa::Actor,
            action == Nessa::Action::"server.read",
            resource is Nessa::Resource
        );"#;
    assert!(
        CedarPolicyEvaluator::from_sources(SCHEMA, &format!(r#"@id("read") {permit}"#)).is_ok()
    );
    assert!(CedarPolicyEvaluator::from_sources(SCHEMA, permit).is_err());
    assert!(CedarPolicyEvaluator::from_sources(SCHEMA, &format!(r#"@id("") {permit}"#)).is_err());
    assert!(CedarPolicyEvaluator::from_sources(SCHEMA, &format!("{POLICIES}\n{permit}")).is_err());
}

#[test]
fn evaluation_diagnostics_fail_closed_even_when_another_policy_permits() {
    let policies = r#"
        @id("read")
        permit(
            principal is Nessa::Actor,
            action == Nessa::Action::"server.read",
            resource is Nessa::Resource
        );
        @id("overflow")
        permit(
            principal is Nessa::Actor,
            action == Nessa::Action::"server.read",
            resource is Nessa::Resource
        ) when { 9223372036854775807 * 2 == 0 };
    "#;
    let evaluator = CedarPolicyEvaluator::from_sources(SCHEMA, policies).unwrap();
    let (context, action, resource, snapshot) = request_values();

    assert_eq!(
        evaluator.evaluate(&context, &action, &resource, &snapshot),
        Ok(Decision::Deny)
    );
}

fn gateway_grant(action: &str) -> Grant {
    Grant::new(
        Action::new(action).unwrap(),
        Resource::new(
            OrganizationId::new("organization-1").unwrap(),
            ResourceId::new("gateway-1").unwrap(),
        ),
    )
}

/// Rows H4 and I1 of `docs/design/auth/peer-gateways.md`: a peer gateway may
/// hold the read grant and `environment.serve` and none of the conversation
/// authority's actions; the other kinds may hold anything but
/// `environment.serve`, which only a peer serves. Unknown actions are held by
/// no one.
#[test]
fn which_grants_each_kind_may_hold() {
    let evaluator = CedarPolicyEvaluator::new().unwrap();
    let hold = |kind, action: &str| {
        evaluator.may_hold(kind, &gateway_grant(action), IssuePath::SurfaceProvision)
    };
    for allowed in ["conversation.read", "environment.serve"] {
        assert_eq!(
            hold(PrincipalKind::Gateway, allowed),
            Ok(Decision::Allow),
            "{allowed}"
        );
    }
    for refused in [
        "conversation.write",
        "credential.manage",
        "server.read",
        "anything.else",
    ] {
        assert_eq!(
            hold(PrincipalKind::Gateway, refused),
            Ok(Decision::Deny),
            "{refused}"
        );
    }
    for kind in [
        PrincipalKind::Human,
        PrincipalKind::Agent,
        PrincipalKind::Integration,
    ] {
        for allowed in [
            "server.read",
            "credential.manage",
            "conversation.read",
            "conversation.write",
        ] {
            assert_eq!(
                hold(kind, allowed),
                Ok(Decision::Allow),
                "{kind:?} {allowed}"
            );
        }
        for refused in ["environment.serve", "anything.else"] {
            assert_eq!(
                hold(kind, refused),
                Ok(Decision::Deny),
                "{kind:?} {refused}"
            );
        }
    }
}

const PATHS: [IssuePath; 6] = [
    IssuePath::Bootstrap,
    IssuePath::ProductIssue,
    IssuePath::WebhookIssue,
    IssuePath::SurfaceProvision,
    IssuePath::OwnerRecovery,
    IssuePath::StoredOnLoad,
];

/// `hold.administration`: administration is refused only on the product
/// path. Webhook issuance further restricts the principal and read grant;
/// other paths retain the kind's own answer.
#[test]
fn administration_and_server_read_respect_product_and_webhook_paths() {
    let evaluator = CedarPolicyEvaluator::new().unwrap();
    let manage = gateway_grant("credential.manage");
    let read = gateway_grant("server.read");
    for path in PATHS {
        let expected = if matches!(path, IssuePath::ProductIssue | IssuePath::WebhookIssue) {
            Decision::Deny
        } else {
            Decision::Allow
        };
        assert_eq!(
            evaluator.may_hold(PrincipalKind::Human, &manage, path),
            Ok(expected),
            "{path:?}"
        );
        assert_eq!(
            evaluator.may_hold(PrincipalKind::Human, &read, path),
            Ok(if path == IssuePath::WebhookIssue {
                Decision::Deny
            } else {
                Decision::Allow
            }),
            "{path:?}"
        );
    }
}

/// `issue.member` and `issue.admin`: a member may be issued a credential on
/// ordinary paths; the webhook path requires an integration member. An admin
/// requires bootstrap, provisioning or recovery; only pairing enrolls a peer.
#[test]
fn who_may_be_issued_a_credential_on_each_path() {
    let evaluator = CedarPolicyEvaluator::new().unwrap();
    let gateway = gateway_grant("server.read").resource().clone();
    let issue = |kind, role, path| evaluator.may_issue(kind, role, &gateway, path);
    for path in PATHS {
        for kind in [
            PrincipalKind::Human,
            PrincipalKind::Agent,
            PrincipalKind::Integration,
        ] {
            assert_eq!(
                issue(kind, MembershipRole::Member, path),
                Ok(
                    if path == IssuePath::WebhookIssue && kind != PrincipalKind::Integration {
                        Decision::Deny
                    } else {
                        Decision::Allow
                    }
                ),
                "{kind:?} member {path:?}"
            );
            let admin = if matches!(
                path,
                IssuePath::Bootstrap | IssuePath::SurfaceProvision | IssuePath::OwnerRecovery
            ) {
                Decision::Allow
            } else {
                Decision::Deny
            };
            assert_eq!(
                issue(kind, MembershipRole::Admin, path),
                Ok(admin),
                "{kind:?} admin {path:?}"
            );
        }
        for role in [MembershipRole::Member, MembershipRole::Admin] {
            assert_eq!(
                issue(PrincipalKind::Gateway, role, path),
                Ok(Decision::Deny),
                "gateway {role:?} {path:?}"
            );
        }
    }
}

#[test]
fn a_role_without_the_grant_is_denied_and_the_grant_without_the_role_too() {
    let (context, _, resource, mut snapshot) = request_values();
    let evaluator = CedarPolicyEvaluator::new().unwrap();
    let manage = Action::new("credential.manage").unwrap();
    // A member holding the grant: the role refuses it.
    snapshot.credential = Credential::new(
        context.credential_id().clone(),
        context.principal_id().clone(),
        context.organization_id().clone(),
        context.audience_id().clone(),
        100,
        200,
        vec![Grant::new(manage.clone(), resource.clone())],
    )
    .unwrap();
    assert_eq!(
        evaluator.evaluate(&context, &manage, &resource, &snapshot),
        Ok(Decision::Deny)
    );
    // An admin without the grant: the missing grant refuses it.
    snapshot.membership = Membership::new(
        context.membership_id().clone(),
        context.principal_id().clone(),
        context.organization_id().clone(),
        MembershipRole::Admin,
        MembershipStatus::Active,
    );
    let read = Action::new("server.read").unwrap();
    assert_eq!(
        evaluator.evaluate(&context, &read, &resource, &snapshot),
        Ok(Decision::Deny)
    );
    assert_eq!(
        evaluator.evaluate(&context, &manage, &resource, &snapshot),
        Ok(Decision::Allow)
    );
}

#[test]
fn an_action_the_schema_does_not_give_an_actor_is_denied_not_unavailable() {
    let (context, _, resource, snapshot) = request_values();
    let evaluator = CedarPolicyEvaluator::new().unwrap();
    for action in ["anything.else", "credential.hold", "credential.issue"] {
        assert_eq!(
            evaluator.evaluate(
                &context,
                &Action::new(action).unwrap(),
                &resource,
                &snapshot
            ),
            Ok(Decision::Deny),
            "{action}"
        );
    }
}

#[test]
fn another_identitys_snapshot_is_denied() {
    let (context, action, resource, mut snapshot) = request_values();
    let evaluator = CedarPolicyEvaluator::new().unwrap();
    snapshot.membership = Membership::new(
        MembershipId::new("membership-2").unwrap(),
        context.principal_id().clone(),
        context.organization_id().clone(),
        MembershipRole::Member,
        MembershipStatus::Active,
    );
    assert_eq!(
        evaluator.evaluate(&context, &action, &resource, &snapshot),
        Ok(Decision::Deny)
    );
}

#[test]
fn webhook_path_holds_only_an_integration_conversation_read_grant() {
    let evaluator = CedarPolicyEvaluator::new().unwrap();
    for kind in [
        PrincipalKind::Human,
        PrincipalKind::Agent,
        PrincipalKind::Integration,
        PrincipalKind::Gateway,
    ] {
        for action in [
            "conversation.read",
            "conversation.write",
            "credential.manage",
            "server.read",
            "environment.serve",
            "anything.else",
        ] {
            let expected = if kind == PrincipalKind::Integration && action == "conversation.read" {
                Decision::Allow
            } else {
                Decision::Deny
            };
            assert_eq!(
                evaluator.may_hold(kind, &gateway_grant(action), IssuePath::WebhookIssue),
                Ok(expected),
                "{kind:?} {action}"
            );
        }
    }
}

#[test]
fn session_entry_uses_original_issuance_and_current_identity() {
    use crate::{
        application::{dto::CredentialTransitionDto, origin::CredentialOrigin},
        domain::{Initiator, IssuanceCause},
    };
    let policy = CedarPolicyEvaluator::new().unwrap();
    let (context, _, resource, mut snapshot) = request_values();
    let action = Action::new("session.enter").unwrap();
    for cause in [
        IssuanceCause::Bootstrap,
        IssuanceCause::AdminIssue,
        IssuanceCause::WebhookIssue,
        IssuanceCause::DevicePairing,
        IssuanceCause::SurfaceProvision,
        IssuanceCause::OwnerRecovery,
    ] {
        let issued = snapshot
            .credential
            .issued(
                cause,
                Initiator::Principal(PrincipalId::new("owner").unwrap()),
            )
            .unwrap();
        snapshot.origin = CredentialOrigin::restore(
            CredentialTransitionDto::record(&issued, 1, 1, Some("request".into())),
            &snapshot.credential,
        )
        .unwrap();
        assert_eq!(
            policy
                .evaluate(&context, &action, &resource, &snapshot)
                .unwrap(),
            if cause == IssuanceCause::WebhookIssue {
                Decision::Deny
            } else {
                Decision::Allow
            }
        );
    }
    snapshot.revision = 0;
    assert_eq!(
        policy
            .evaluate(&context, &action, &resource, &snapshot)
            .unwrap(),
        Decision::Deny
    );
    snapshot.revision = 1;
    snapshot.membership = Membership::new(
        snapshot.membership.id().clone(),
        snapshot.membership.principal_id().clone(),
        snapshot.membership.organization_id().clone(),
        MembershipRole::Member,
        MembershipStatus::Disabled,
    );
    assert_eq!(
        policy
            .evaluate(&context, &action, &resource, &snapshot)
            .unwrap(),
        Decision::Deny
    );
}
