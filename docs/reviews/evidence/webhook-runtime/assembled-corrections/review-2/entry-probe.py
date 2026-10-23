from pathlib import Path
import json,os,subprocess
p=Path('/tmp/nessa-assembled-final-review');f=p/'crates/nessa-auth/src/adapters/cedar/evaluator_tests.rs'
s='''
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
'''
(p/'entry-probe.rs').write_text(s);f.write_text(f.read_text()+s)
a=json.loads((p/'auth-resumed-args.json').read_text());i=a.index('--crate-type');del a[i:i+2];a.insert(1,'--test');a[a.index('-o')+1]=str(p/'auth-probe-tests');env=dict(os.environ,CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
with(p/'entry-probe-compile.txt').open('w')as out:subprocess.run(a,stdout=out,stderr=subprocess.STDOUT,check=True,env=env)
with(p/'entry-probe.txt').open('w')as out:subprocess.run([str(p/'auth-probe-tests'),'independent_entry_origin','--nocapture'],stdout=out,stderr=subprocess.STDOUT,check=True,env=env)
print((p/'entry-probe.txt').read_text())
