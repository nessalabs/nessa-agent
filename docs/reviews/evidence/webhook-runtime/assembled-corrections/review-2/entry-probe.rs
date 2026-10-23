
#[test]
fn independent_entry_origin_crosses_role_and_membership_state() {
 use crate::{application::{dto::CredentialTransitionDto,origin::CredentialOrigin},domain::{Initiator,IssuanceCause}};
 let policy=CedarPolicyEvaluator::new().unwrap();
 for role in [MembershipRole::Member,MembershipRole::Admin] {
  for state in [MembershipStatus::Active,MembershipStatus::Disabled] {
   for cause in [IssuanceCause::Bootstrap,IssuanceCause::AdminIssue,IssuanceCause::WebhookIssue,IssuanceCause::DevicePairing,IssuanceCause::SurfaceProvision,IssuanceCause::OwnerRecovery] {
    let (context,_,resource,mut snapshot)=request_values();
    snapshot.membership=Membership::new(snapshot.membership.id().clone(),snapshot.membership.principal_id().clone(),snapshot.membership.organization_id().clone(),role,state);
    let original=snapshot.credential.issued(cause,Initiator::Principal(PrincipalId::new("original-issuer").unwrap())).unwrap();
    snapshot.origin=CredentialOrigin::restore(CredentialTransitionDto::record(&original,1,1,Some("original-command".into())),&snapshot.credential).unwrap();
    let expected=if state==MembershipStatus::Active&&cause!=IssuanceCause::WebhookIssue {Decision::Allow}else{Decision::Deny};
    assert_eq!(policy.evaluate(&context,&Action::new("session.enter").unwrap(),&resource,&snapshot).unwrap(),expected,"{role:?}/{state:?}/{cause:?}");
   }
  }
 }
}
