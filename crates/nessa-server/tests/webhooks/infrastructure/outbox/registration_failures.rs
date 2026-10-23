use super::*;
use nessa_auth::application::{
    credential_admin::{CredentialAdmin, RevokeCredentialOutcome},
    dto::{CredentialMetadataDto, CredentialTransitionDto, IssuanceCauseDto, TransitionCauseDto},
    ports::{AccessAdmission, AccessError, AccessHold, PortFuture},
};
use nessa_auth::domain::{Membership, MembershipId};

type Alter = fn(&mut CredentialMetadataDto, &mut Vec<CredentialTransitionDto>);
struct ChangedIssue<'a> {
    auth: &'a LocalCredentialStore,
    alter: Alter,
}
impl CredentialAdmin for ChangedIssue<'_> {
    fn issue<'a>(
        &'a self,
        request: IssueCredentialRequest,
    ) -> PortFuture<'a, IssueCredentialOutcome, CredentialAdminError> {
        Box::pin(async move {
            let mut outcome = self.auth.issue(request).await?;
            match &mut outcome {
                IssueCredentialOutcome::Issued {
                    metadata,
                    transitions,
                    ..
                }
                | IssueCredentialOutcome::ExistingSecretUnavailable {
                    metadata,
                    transitions,
                } => (self.alter)(metadata, transitions),
            }
            Ok(outcome)
        })
    }
    fn list<'a>(
        &'a self,
        request: ListCredentialsRequest,
    ) -> PortFuture<'a, Vec<CredentialMetadataDto>, CredentialAdminError> {
        self.auth.list(request)
    }
    fn revoke<'a>(
        &'a self,
        request: RevokeCredentialRequest,
    ) -> PortFuture<'a, RevokeCredentialOutcome, CredentialAdminError> {
        self.auth.revoke(request)
    }
}

#[tokio::test]
async fn contradictory_auth_reply_installs_nothing_and_preserves_original_retry() {
    let cases: [(&str, Alter); 17] = [
        ("credential", |m, _| m.id = "different-key".into()),
        ("principal", |m, _| {
            m.principal_id = "different-principal".into()
        }),
        ("organization", |m, _| {
            m.organization_id = "different-org".into()
        }),
        ("audience", |m, _| {
            m.audience_id = "different-gateway".into()
        }),
        ("issued_at", |m, _| m.issued_at += 1),
        ("expiry", |m, _| m.expires_at = Some(99)),
        ("grants", |m, _| m.grants.push(grant("conversation.write"))),
        ("transition_target", |_, t| {
            t[0].credential_id = "different-key".into()
        }),
        ("original_caller", |_, t| {
            t[0].initiator = InitiatorDto::Principal {
                id: "different-owner".into(),
            }
        }),
        ("correlation", |_, t| {
            t[0].correlation = Some("different-request".into())
        }),
        ("transition_time", |_, t| t[0].at += 1),
        ("before", |_, t| t[0].before = Some(t[0].after)),
        ("after", |_, t| t[0].after.expires_at = Some(99)),
        ("after_revoked", |_, t| t[0].after.revoked_at = Some(11)),
        ("missing_transition", |_, t| *t = Vec::new()),
        ("cause", |_, t| {
            t[0].cause = TransitionCauseDto::Issued {
                cause: IssuanceCauseDto::Bootstrap,
            }
        }),
        ("extra_transition", |_, t| t.push(t[0].clone())),
    ];
    for (name, alter) in cases {
        let f = Registered::new();
        let proposal = f.proposed();
        let id = proposal.id;
        let changed = ChangedIssue {
            auth: &f.auth,
            alter,
        };
        let mut command = f.command(&f.store);
        command.credentials = &changed;
        assert!(
            matches!(
                command.execute(proposal).await,
                Err(RegistrationFailure::Inconsistent)
            ),
            "{name}"
        );
        assert_eq!(f.count("registrations"), 0, "{name}");
        assert_eq!(f.count("registration_intents"), 1, "{name}");
        let IssueCredentialOutcome::ExistingSecretUnavailable { metadata, .. } =
            f.command(&f.store).execute(f.proposed()).await.unwrap()
        else {
            panic!("cannot replay discarded bearer")
        };
        assert_eq!(
            f.store
                .registration(CredentialId::new(metadata.id).unwrap())
                .await
                .unwrap()
                .unwrap()
                .id,
            id,
            "{name}"
        );
    }
}

#[tokio::test]
async fn contradictory_existing_receipt_does_not_discard_the_known_issued_intent() {
    let f = Registered::new();
    let intent = f.store.reserve(f.proposed()).await.unwrap();
    let id = intent.id;
    let _issued = f.auth.issue_sync(intent.issue).unwrap();
    let changed = ChangedIssue {
        auth: &f.auth,
        alter: |metadata, _| metadata.id = "invented-replay".into(),
    };
    let mut command = f.command(&f.store);
    command.credentials = &changed;
    assert!(matches!(
        command.execute(f.proposed()).await,
        Err(RegistrationFailure::Inconsistent)
    ));
    assert_eq!(f.count("registration_intents"), 1);
    assert_eq!(f.count("registrations"), 0);
    let IssueCredentialOutcome::ExistingSecretUnavailable { metadata, .. } =
        f.command(&f.store).execute(f.proposed()).await.unwrap()
    else {
        panic!("original secret stays unavailable")
    };
    assert_eq!(
        f.store
            .registration(CredentialId::new(metadata.id).unwrap())
            .await
            .unwrap()
            .unwrap()
            .id,
        id
    );
}

struct FailedIssue<'a> {
    auth: &'a LocalCredentialStore,
    error: CredentialAdminError,
    commit: bool,
}
impl CredentialAdmin for FailedIssue<'_> {
    fn issue<'a>(
        &'a self,
        request: IssueCredentialRequest,
    ) -> PortFuture<'a, IssueCredentialOutcome, CredentialAdminError> {
        Box::pin(async move {
            if self.commit {
                drop(self.auth.issue(request).await?);
            }
            Err(self.error)
        })
    }
    fn list<'a>(
        &'a self,
        request: ListCredentialsRequest,
    ) -> PortFuture<'a, Vec<CredentialMetadataDto>, CredentialAdminError> {
        self.auth.list(request)
    }
    fn revoke<'a>(
        &'a self,
        request: RevokeCredentialRequest,
    ) -> PortFuture<'a, RevokeCredentialOutcome, CredentialAdminError> {
        self.auth.revoke(request)
    }
}

#[tokio::test]
async fn uncertain_auth_commit_keeps_original_intent_and_typed_refusal_discards_only_unissued() {
    for (error, commit) in [
        (CredentialAdminError::Unavailable, false),
        (CredentialAdminError::Unavailable, true),
        (CredentialAdminError::Capacity, false),
        (CredentialAdminError::Conflict, false),
    ] {
        let f = Registered::new();
        let proposed = f.proposed();
        let id = proposed.id;
        let failed = FailedIssue {
            auth: &f.auth,
            error,
            commit,
        };
        let mut command = f.command(&f.store);
        command.credentials = &failed;
        assert!(
            matches!(command.execute(proposed).await,Err(RegistrationFailure::Auth(observed)) if observed==error)
        );
        assert_eq!(f.count("registrations"), 0);
        assert_eq!(
            f.count("registration_intents"),
            i64::from(error == CredentialAdminError::Unavailable)
        );
        let outcome = f.command(&f.store).execute(f.proposed()).await.unwrap();
        let metadata = match outcome {
            IssueCredentialOutcome::Issued { metadata, .. } => {
                assert!(!commit);
                metadata
            }
            IssueCredentialOutcome::ExistingSecretUnavailable { metadata, .. } => {
                assert!(commit);
                metadata
            }
        };
        let installed = f
            .store
            .registration(CredentialId::new(metadata.id).unwrap())
            .await
            .unwrap()
            .unwrap();
        if error == CredentialAdminError::Unavailable {
            assert_eq!(installed.id, id);
        }
    }
}

struct FailedHold(AccessError);
impl AccessAdmission for FailedHold {
    fn hold<'a>(&'a self, _: &'a CredentialId) -> PortFuture<'a, AccessHold> {
        Box::pin(async move { Err(self.0) })
    }
}
struct ChangedMembership<'a>(&'a LocalCredentialStore);
impl AccessAdmission for ChangedMembership<'_> {
    fn hold<'a>(&'a self, id: &'a CredentialId) -> PortFuture<'a, AccessHold> {
        Box::pin(async move {
            let mut hold = self.0.hold(id).await?;
            let current = &hold.snapshot.membership;
            hold.snapshot.membership = Membership::new(
                MembershipId::new("different-membership").unwrap(),
                current.principal_id().clone(),
                current.organization_id().clone(),
                current.role(),
                current.status(),
            );
            Ok(hold)
        })
    }
}

#[tokio::test]
async fn unavailable_or_substituted_current_authority_cannot_install_or_erase_intent() {
    for error in [
        AccessError::Unavailable,
        AccessError::StaleRevision,
        AccessError::Unsupported,
    ] {
        let f = Registered::new();
        let failed = FailedHold(error);
        let mut command = f.command(&f.store);
        command.admission = &failed;
        assert!(matches!(
            command.execute(f.proposed()).await,
            Err(RegistrationFailure::Auth(CredentialAdminError::Unavailable))
        ));
        assert_eq!(f.count("registrations"), 0);
        assert_eq!(f.count("registration_intents"), 1);
        assert!(matches!(
            f.command(&f.store).execute(f.proposed()).await,
            Ok(IssueCredentialOutcome::ExistingSecretUnavailable { .. })
        ));
    }
    let f = Registered::new();
    let changed = ChangedMembership(&f.auth);
    let mut command = f.command(&f.store);
    command.admission = &changed;
    assert!(matches!(
        command.execute(f.proposed()).await,
        Err(RegistrationFailure::Inconsistent)
    ));
    assert_eq!(f.count("registrations"), 0);
    assert_eq!(f.count("registration_intents"), 1);
    assert!(matches!(
        f.command(&f.store).execute(f.proposed()).await,
        Ok(IssueCredentialOutcome::ExistingSecretUnavailable { .. })
    ));
}
