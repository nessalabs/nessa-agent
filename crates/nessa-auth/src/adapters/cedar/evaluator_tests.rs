use super::*;
use crate::domain::{
    AudienceId, Credential, CredentialId, Grant, Membership, MembershipId, MembershipRole,
    MembershipStatus, OrganizationId, PrincipalId, ResourceId,
};

fn request_values() -> (AuthContext, Action, Resource, AccessSnapshot) {
    let principal_id = PrincipalId::new("principal-1").unwrap();
    let organization_id = OrganizationId::new("organization-1").unwrap();
    let membership_id = MembershipId::new("membership-1").unwrap();
    let credential_id = CredentialId::new("credential-1").unwrap();
    let audience_id = AudienceId::new("gateway-1").unwrap();
    let action = Action::new(SERVER_READ).unwrap();
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
