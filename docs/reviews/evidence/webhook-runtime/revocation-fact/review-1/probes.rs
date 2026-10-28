#[test]
fn independent_later_command_cannot_replace_original_fact_or_journal_identity() {
    ready(async {
        let mut f = Fixture::new();
        let _ = f.issue(CredentialIssuePurpose::Webhook);
        let id = CredentialId::new("synthetic-hook").unwrap();
        let first = f.store().revoke_sync(RevokeCredentialRequest {
            request_id: "original-revoke".into(), issuer_principal_id: "owner".into(),
            credential_id: id.as_str().into(), revoked_at: 40,
        }).unwrap();
        let original = f.store().read(&id).await.unwrap();
        let fact = original.revocation_fact().unwrap().unwrap().clone();
        let later = f.store().revoke_sync(RevokeCredentialRequest {
            request_id: "different-command".into(), issuer_principal_id: "owner".into(),
            credential_id: id.as_str().into(), revoked_at: 0,
        }).unwrap();
        assert!(later.revision > first.revision);
        assert_eq!(later.revocation, first.revocation);
        let after = f.store().read(&id).await.unwrap();
        assert_eq!(after.revocation_fact().unwrap(),Some(&fact));
        assert!(after.revision > fact.revision());
        assert_eq!(fact.correlation(),Some("original-revoke"));
        assert_eq!(fact.transition().at(),40);
        f.reopen();
        assert_eq!(f.store().read(&id).await.unwrap().revocation_fact().unwrap(),Some(&fact));
    });
}

#[test]
fn independent_pending_revocation_waits_for_existing_hold_then_returns_original_fact() {
    use nessa_auth::application::credential_admin::CredentialAdmin;
    use std::task::{Context,Poll,Waker};
    ready(async {
        let f = Fixture::new(); let _ = f.issue(CredentialIssuePurpose::Webhook);
        let id=CredentialId::new("synthetic-hook").unwrap();
        let held=f.store().hold(&id).await.unwrap();
        let saved=held.snapshot.clone();
        let mut revoke=f.store().revoke(RevokeCredentialRequest {
            request_id:"held-revoke".into(),issuer_principal_id:"owner".into(),
            credential_id:id.as_str().into(),revoked_at:50,
        });
        let mut cx=Context::from_waker(Waker::noop());
        assert!(matches!(revoke.as_mut().poll(&mut cx),Poll::Pending));
        assert!(f.store().read(&id).await.unwrap().revocation_fact().unwrap().is_none());
        drop(held);
        let result=match revoke.as_mut().poll(&mut cx) {Poll::Ready(Ok(v))=>v,_=>panic!("released hold must admit pending mutation")};
        let current=f.store().hold(&id).await.unwrap();
        let fact=current.snapshot.revocation_fact().unwrap().unwrap();
        assert_eq!(fact.sequence(),result.revocation.sequence);
        assert_eq!(fact.correlation(),Some("held-revoke"));
        assert_eq!(saved.revocation_fact().unwrap(),None);
        assert!(saved.credential.revoked_at().is_none());
        assert_eq!(current.snapshot.credential.revoked_at(),Some(50));
    });
}

#[test]
fn independent_correlation_byte_boundary_and_original_origin_decoder_agree() {
    let (snapshot,record)=synthetic();
    for correlation in ["é".repeat(100),format!("{}x","a".repeat(199))] {
        let mut changed=record.clone();changed.correlation=Some(correlation.clone());
        let fact=CredentialRevocation::restore(changed,&snapshot.credential).unwrap();
        assert_eq!(fact.correlation(),Some(correlation.as_str()));
    }
    let mut too_many_bytes=record.clone();too_many_bytes.correlation=Some("é".repeat(101));
    assert_eq!(CredentialRevocation::restore(too_many_bytes,&snapshot.credential),Err(AccessError::Unavailable));
    let mut active=snapshot.credential.clone();
    active=Credential::new(active.id().clone(),active.principal_id().clone(),active.organization_id().clone(),active.audience_id().clone(),active.issued_at(),active.expires_at(),vec![]).unwrap();
    let origin=active.issued(IssuanceCause::AdminIssue,Initiator::LocalOperator).unwrap();
    for correlation in [None,Some("é".repeat(100))] {
        let dto=CredentialTransitionDto::record(&origin,1,1,correlation.clone());
        assert_eq!(CredentialOrigin::restore(dto,&active).unwrap().correlation(),correlation.as_deref());
    }
}
