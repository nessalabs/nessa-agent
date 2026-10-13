use super::*;
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
    (
        context,
        action,
        resource,
        AccessSnapshot {
            credential,
            membership,
            revision: 1,
        },
    )
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
        r#"permit(principal, action == Nessa::Action::"unknown", resource);"#,
    )
    .is_err());
}

#[test]
fn evaluation_diagnostics_fail_closed_even_when_another_policy_permits() {
    let policies = r#"
        permit(
            principal is Nessa::Actor,
            action == Nessa::Action::"server.read",
            resource is Nessa::Resource
        );
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
    let hold = |kind, action: &str| evaluator.may_hold(kind, &gateway_grant(action), true);
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

#[test]
fn administration_is_held_only_where_the_issuing_path_may_hand_it_out() {
    let evaluator = CedarPolicyEvaluator::new().unwrap();
    let manage = gateway_grant("credential.manage");
    assert_eq!(
        evaluator.may_hold(PrincipalKind::Human, &manage, true),
        Ok(Decision::Allow)
    );
    assert_eq!(
        evaluator.may_hold(PrincipalKind::Human, &manage, false),
        Ok(Decision::Deny)
    );
    assert_eq!(
        evaluator.may_hold(PrincipalKind::Human, &gateway_grant("server.read"), false),
        Ok(Decision::Allow)
    );
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
    for action in ["anything.else", "credential.hold"] {
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
