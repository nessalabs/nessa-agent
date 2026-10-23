use super::*;
use nessa_auth::application::{
    credential_admin::{CredentialAdmin, RevokeCredentialOutcome},
    dto::{CredentialMetadataDto, CredentialTransitionDto, IssuanceCauseDto, TransitionCauseDto},
    ports::{AccessAdmission, AccessError, AccessHold, PortFuture},
};
use nessa_auth::domain::{Membership, MembershipId, MembershipStatus};

type Alter = fn(&mut CredentialMetadataDto, &mut Vec<CredentialTransitionDto>);
struct ChangedIssue<'a> {
    auth: &'a LocalCredentialStore,
    alter: Alter,
    rewrite: Option<fn(&mut IssueCredentialRequest)>,
}
impl CredentialAdmin for ChangedIssue<'_> {
    fn issue<'a>(
        &'a self,
        mut request: IssueCredentialRequest,
    ) -> PortFuture<'a, IssueCredentialOutcome, CredentialAdminError> {
        Box::pin(async move {
            if let Some(rewrite) = self.rewrite {
                rewrite(&mut request);
            }
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
    let cases: [(&str, Alter); 19] = [
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
        ("journal_sequence", |_, t| t[0].sequence += 1),
        ("journal_revision", |_, t| t[0].revision += 1),
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
            rewrite: None,
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
        let metadata = f
            .command(&f.store)
            .execute(f.proposed())
            .await
            .unwrap()
            .credential;
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
        rewrite: None,
    };
    let mut command = f.command(&f.store);
    command.credentials = &changed;
    assert!(matches!(
        command.execute(f.proposed()).await,
        Err(RegistrationFailure::Inconsistent)
    ));
    assert_eq!(f.count("registration_intents"), 1);
    assert_eq!(f.count("registrations"), 0);
    let metadata = f
        .command(&f.store)
        .execute(f.proposed())
        .await
        .unwrap()
        .credential;
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
        let metadata = outcome.credential;
        let retry = f
            .auth
            .issue_sync(f.store.reserve(f.proposed()).await.unwrap().issue)
            .unwrap();
        assert!(matches!(
            retry,
            IssueCredentialOutcome::ExistingSecretUnavailable { .. }
        ));
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
struct ChangedMembership<'a>(&'a LocalCredentialStore, bool);
impl AccessAdmission for ChangedMembership<'_> {
    fn hold<'a>(&'a self, id: &'a CredentialId) -> PortFuture<'a, AccessHold> {
        Box::pin(async move {
            let mut hold = self.0.hold(id).await?;
            let current = &hold.snapshot.membership;
            hold.snapshot.membership = Membership::new(
                if self.1 {
                    current.id().clone()
                } else {
                    MembershipId::new("different-membership").unwrap()
                },
                current.principal_id().clone(),
                current.organization_id().clone(),
                current.role(),
                if self.1 {
                    MembershipStatus::Disabled
                } else {
                    current.status()
                },
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
            Ok(RegisteredWebhook { .. })
        ));
    }
    for inactive in [false, true] {
        let f = Registered::new();
        let changed = ChangedMembership(&f.auth, inactive);
        let mut command = f.command(&f.store);
        command.admission = &changed;
        assert!(matches!(
            command.execute(f.proposed()).await,
            Err(failure) if failure == if inactive {RegistrationFailure::Stopped} else {RegistrationFailure::Inconsistent}
        ));
        assert_eq!(f.count("registrations"), 0);
        assert_eq!(f.count("registration_intents"), 1);
        assert!(matches!(
            f.command(&f.store).execute(f.proposed()).await,
            Ok(RegisteredWebhook { .. })
        ));
    }
}

#[tokio::test]
async fn coherent_auth_result_for_a_substituted_command_cannot_install_original_intent() {
    let f = Registered::new();
    let changed = ChangedIssue {
        auth: &f.auth,
        alter: |_, _| {},
        rewrite: Some(|request| {
            request.grants.push(request.grants[0].clone());
        }),
    };
    let mut command = f.command(&f.store);
    command.credentials = &changed;
    assert!(matches!(
        command.execute(f.proposed()).await,
        Err(RegistrationFailure::Inconsistent)
    ));
    assert_eq!(f.count("registration_intents"), 1);
    assert_eq!(f.count("registrations"), 0);
    assert!(matches!(
        f.command(&f.store).execute(f.proposed()).await,
        Err(RegistrationFailure::Auth(CredentialAdminError::Conflict))
    ));
    assert_eq!(f.count("registration_intents"), 0);
}

struct ChangedReservation<'a> {
    store: &'a SqlOutbox,
    alter: fn(&mut RegistrationIntent),
    before: bool,
}
#[async_trait]
impl RegistrationIntents for ChangedReservation<'_> {
    async fn reserve(
        &self,
        mut proposed: RegistrationIntent,
    ) -> Result<RegistrationIntent, OutboxError> {
        if self.before {
            (self.alter)(&mut proposed);
        }
        let mut selected = self.store.reserve(proposed).await?;
        if !self.before {
            (self.alter)(&mut selected);
        }
        Ok(selected)
    }
    async fn install(&self, intent: RegistrationIntent) -> Result<(), OutboxError> {
        RegistrationIntents::install(self.store, intent).await
    }
    async fn discard(&self, id: Uuid) -> Result<(), OutboxError> {
        self.store.discard(id).await
    }
}

#[tokio::test]
async fn substituted_reservation_is_rejected_before_auth_and_installation() {
    type AlterIntent = fn(&mut RegistrationIntent);
    let cases: [(&str, AlterIntent); 17] = [
        ("destination", |i| {
            i.config.destination =
                Destination::parse("https://different-recipient.test/changed").unwrap()
        }),
        ("key", |i| i.config.key = Arc::new(MacKey::new([87; 32]))),
        ("malformed_selection", |i| i.issue.credential_id.clear()),
        ("time_projection", |i| i.issue.issued_at += 1),
        ("request", |i| {
            i.issue.request_id = "different-request".into()
        }),
        ("issuer", |i| {
            i.issue.issuer_principal_id = "different-issuer".into()
        }),
        ("principal", |i| {
            i.issue.principal.id = "different-hook".into();
            i.issue.membership.principal_id = "different-hook".into();
        }),
        ("kind", |i| i.issue.principal.kind = PrincipalKindDto::Human),
        ("membership", |i| {
            i.issue.membership.id = "different-membership".into()
        }),
        ("member_principal", |i| {
            i.issue.membership.principal_id = "different-hook".into()
        }),
        ("organization", |i| {
            i.issue.membership.organization_id = "different-org".into()
        }),
        ("role", |i| {
            i.issue.membership.role = MembershipRoleDto::Admin
        }),
        ("state", |i| {
            i.issue.membership.state = MembershipStateDto::Disabled
        }),
        ("audience", |i| {
            i.issue.audience_id = "different-gateway".into()
        }),
        ("expiry", |i| i.issue.expires_at = Some(99)),
        ("grants", |i| {
            i.issue.grants.push(grant("conversation.write"))
        }),
        ("purpose", |i| {
            i.issue.purpose = CredentialIssuePurpose::Product
        }),
    ];
    for (field, alter) in cases {
        for before in [false, true] {
            // The real reserve owner refuses these malformed proposals. Test
            // their substitution after its valid returned selection instead.
            if before && matches!(field, "purpose" | "malformed_selection" | "time_projection") {
                continue;
            }
            let f = Registered::new();
            let substitute = ChangedReservation {
                store: &f.store,
                alter,
                before,
            };
            assert!(
                matches!(
                    f.command(&substitute).execute(f.proposed()).await,
                    Err(RegistrationFailure::Inconsistent)
                ),
                "{field}, before={before}"
            );
            assert_eq!(f.count("registrations"), 0, "{field}");
            assert_eq!(f.count("registration_intents"), 1, "{field}");
            let current = f
                .command(&f.store)
                .credentials
                .list(ListCredentialsRequest {
                    organization_id: "org".into(),
                })
                .await
                .unwrap();
            assert_eq!(current.len(), 1, "no auth issuance for {field}");
            assert_eq!(current[0].id, "owner-key");
        }
    }
}

#[tokio::test]
async fn transient_intent_scan_interruption_is_unavailable_and_unchanged_state_recovers() {
    struct Counter {
        steps: usize,
        stop: usize,
    }
    unsafe extern "C" fn interrupt(context: *mut std::ffi::c_void) -> std::ffi::c_int {
        let counter = unsafe { &mut *(context as *mut Counter) };
        counter.steps += 1;
        i32::from(counter.steps >= counter.stop)
    }
    let f = Registered::new();
    f.store.reserve(f.proposed()).await.unwrap();
    let connection = f.store.owner.connection.lock().unwrap();
    let mut interrupted = 0;
    for stop in 1..=150 {
        let mut counter = Counter { steps: 0, stop };
        unsafe {
            nessa_local_database::rusqlite::ffi::sqlite3_progress_handler(
                connection.handle(),
                1,
                Some(interrupt),
                &mut counter as *mut Counter as *mut std::ffi::c_void,
            );
        }
        let result = intents::validate(&connection);
        unsafe {
            nessa_local_database::rusqlite::ffi::sqlite3_progress_handler(
                connection.handle(),
                0,
                None,
                std::ptr::null_mut(),
            );
        }
        match result {
            Ok(()) => (),
            Err(OutboxError::Unavailable) => interrupted += 1,
            other => panic!("transient scan failed as {other:?} at step {stop}"),
        }
        assert_eq!(intents::validate(&connection), Ok(()));
    }
    assert!(interrupted > 0);
}

#[tokio::test]
async fn actual_original_command_cannot_be_replaced_by_an_honest_looking_reply() {
    let f = Registered::new();
    let changed = ChangedIssue {
        auth: &f.auth,
        rewrite: Some(|request| request.request_id = "actual-other-command".into()),
        alter: |_, transitions| transitions[0].correlation = Some("register-synthetic".into()),
    };
    let mut command = f.command(&f.store);
    command.credentials = &changed;
    assert_eq!(
        command.execute(f.proposed()).await,
        Err(RegistrationFailure::Inconsistent)
    );
    assert_eq!(f.count("registrations"), 0);
    assert_eq!(f.count("registration_intents"), 1);
    let records = f
        .auth
        .list_transitions_sync(
            &nessa_auth::application::credential_admin::ListTransitionsRequest {
                organization_id: "org".into(),
            },
        )
        .unwrap();
    assert_eq!(
        records.last().unwrap().correlation.as_deref(),
        Some("actual-other-command")
    );
}

#[tokio::test]
async fn ordinary_origin_requires_explicit_recovery_even_if_reply_claims_webhook() {
    let f = Registered::new();
    let changed = ChangedIssue {
        auth: &f.auth,
        rewrite: Some(|request| request.purpose = CredentialIssuePurpose::Product),
        alter: |_, transitions| {
            transitions[0].cause = TransitionCauseDto::Issued {
                cause: IssuanceCauseDto::WebhookIssue,
            }
        },
    };
    let mut command = f.command(&f.store);
    command.credentials = &changed;
    assert_eq!(
        command.execute(f.proposed()).await,
        Err(RegistrationFailure::OriginRecoveryRequired)
    );
    assert_eq!(f.count("registrations"), 0);
    assert_eq!(f.count("registration_intents"), 1);
}

#[tokio::test]
async fn actual_original_actor_cannot_be_replaced_by_a_claimed_reply_actor() {
    let f = Registered::new();
    let mut other = f.proposed().issue;
    other.purpose = CredentialIssuePurpose::Product;
    other.request_id = "create-second-test-issuer".into();
    other.principal.id = "second-test-issuer".into();
    other.membership.id = "second-test-membership".into();
    other.membership.principal_id = "second-test-issuer".into();
    drop(f.auth.issue_sync(other).unwrap());
    let changed = ChangedIssue {
        auth: &f.auth,
        rewrite: Some(|request| request.issuer_principal_id = "second-test-issuer".into()),
        alter: |_, transitions| {
            transitions[0].initiator = InitiatorDto::Principal { id: "owner".into() }
        },
    };
    let mut command = f.command(&f.store);
    command.credentials = &changed;
    assert_eq!(
        command.execute(f.proposed()).await,
        Err(RegistrationFailure::Inconsistent)
    );
    assert_eq!(f.count("registrations"), 0);
    assert_eq!(f.count("registration_intents"), 1);
}
