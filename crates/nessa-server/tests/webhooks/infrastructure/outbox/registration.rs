use super::super::super::intents;
use super::*;
use crate::webhooks::application::{
    RegisterWebhook, RegisteredWebhook, RegistrationConfig, RegistrationFailure,
    RegistrationIntent, RegistrationIntents,
};
use async_trait::async_trait;
use nessa_auth::{
    adapters::{
        cedar::CedarPolicyEvaluator,
        local::{BootstrapRequest, LocalCredentialStore},
    },
    application::{
        credential_admin::{
            CredentialAdminError, CredentialIssuePurpose, IssueCredentialOutcome,
            IssueCredentialRequest, ListCredentialsRequest, RevokeCredentialRequest,
        },
        dto::{
            CredentialGrantDto, InitiatorDto, MembershipInputDto, MembershipRoleDto,
            MembershipStateDto, OrganizationInputDto, PrincipalInputDto, PrincipalKindDto,
            ResourceDto,
        },
    },
};

struct Registered {
    _root: tempfile::TempDir,
    auth: LocalCredentialStore,
    store: SqlOutbox,
    clock: Arc<Time>,
    path: PathBuf,
    audit: PathBuf,
}
impl Registered {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let private = root.path().join("private");
        nessa_local_storage::create_directory(&private).unwrap();
        let auth = LocalCredentialStore::open(
            &private,
            "auth.json",
            Arc::new(CedarPolicyEvaluator::new().unwrap()),
        )
        .unwrap();
        let _boot = auth
            .bootstrap(BootstrapRequest {
                gateway_id: "gateway".into(),
                organization: OrganizationInputDto { id: "org".into() },
                principal: PrincipalInputDto {
                    id: "owner".into(),
                    kind: PrincipalKindDto::Human,
                },
                membership: MembershipInputDto {
                    id: "owner-member".into(),
                    principal_id: "owner".into(),
                    organization_id: "org".into(),
                    role: MembershipRoleDto::Admin,
                    state: MembershipStateDto::Active,
                },
                credential_id: "owner-key".into(),
                issued_at: 1,
                expires_at: None,
                grants: vec![grant("credential.manage")],
            })
            .unwrap();
        let clock = Arc::new(Time(AtomicU64::new(10_000)));
        let path = private.join("outbox.db");
        let audit = private.join("audit");
        let store = SqlOutbox::open(&path, audit.clone(), clock.clone()).unwrap();
        Self {
            _root: root,
            auth,
            store,
            clock,
            path,
            audit,
        }
    }
    fn proposed(&self) -> RegistrationIntent {
        RegistrationIntent {
            id: Uuid::new_v4(),
            observed_at_ms: 10_000,
            issue: IssueCredentialRequest {
                purpose: CredentialIssuePurpose::Webhook,
                request_id: "register-synthetic".into(),
                issuer_principal_id: "owner".into(),
                credential_id: format!("credential-{}", Uuid::new_v4()),
                principal: PrincipalInputDto {
                    id: "hook".into(),
                    kind: PrincipalKindDto::Integration,
                },
                membership: MembershipInputDto {
                    id: "hook-member".into(),
                    principal_id: "hook".into(),
                    organization_id: "org".into(),
                    role: MembershipRoleDto::Member,
                    state: MembershipStateDto::Active,
                },
                audience_id: "gateway".into(),
                issued_at: 10,
                expires_at: Some(100),
                grants: vec![grant("conversation.read")],
            },
            config: RegistrationConfig {
                destination: Destination::parse("https://recipient.test/private-only-fixture")
                    .unwrap(),
                key: Arc::new(MacKey::new([29; 32])),
            },
        }
    }
    fn command<'a>(&'a self, intents: &'a dyn RegistrationIntents) -> RegisterWebhook<'a> {
        RegisterWebhook {
            credentials: &self.auth,
            admission: &self.auth,
            intents,
            stops: &self.store,
            clock: self.clock.as_ref(),
        }
    }
    fn count(&self, table: &str) -> i64 {
        assert!(["registration_intents", "registrations"].contains(&table));
        self.store
            .owner
            .connection
            .lock()
            .unwrap()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
    fn reopen(&mut self) {
        // Replace the handle only after its canonical lock owner is dropped.
        let replacement_root = tempfile::tempdir().unwrap();
        let replacement_private = replacement_root.path().join("private");
        nessa_local_storage::create_directory(&replacement_private).unwrap();
        let placeholder = SqlOutbox::open(
            &replacement_private.join("unused.db"),
            replacement_private.join("audit"),
            self.clock.clone(),
        )
        .unwrap();
        drop(std::mem::replace(&mut self.store, placeholder));
        self.store = SqlOutbox::open(&self.path, self.audit.clone(), self.clock.clone()).unwrap();
    }
}

fn grant(action: &str) -> CredentialGrantDto {
    CredentialGrantDto {
        action: action.into(),
        resource: ResourceDto {
            organization_id: "org".into(),
            id: "gateway".into(),
        },
    }
}

#[path = "registration_origin.rs"]
mod origin_tests;

#[path = "registration_failures.rs"]
mod failure_tests;

#[tokio::test]
async fn registration_commits_real_auth_and_replays_original_config_after_restart() {
    let mut f = Registered::new();
    let mut proposal = f.proposed();
    proposal.issue.purpose = CredentialIssuePurpose::Product;
    proposal.issue.issued_at = 999;
    let id = proposal.id;
    let credential = proposal.issue.credential_id.clone();
    let acknowledgement = f.command(&f.store).execute(proposal).await.unwrap();
    let metadata = acknowledgement.credential.clone();
    assert_eq!(acknowledgement.registration_id, id);
    assert_eq!(metadata.id, credential);
    let saved = f
        .store
        .registration(CredentialId::new(&credential).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.id, id);
    assert_eq!(saved.key.expose_bytes(), &[29; 32]);
    assert!(f
        .store
        .owner
        .connection
        .lock()
        .unwrap()
        .execute(
            "UPDATE registration_intents SET request='substituted-request'",
            []
        )
        .is_err());
    assert_eq!(f.store.discard(id).await, Err(OutboxError::Conflict));
    let logs = std::fs::read_dir(&f.audit)
        .unwrap()
        .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!logs.contains("nessa_v1."));
    assert!(!format!("{acknowledgement:?}").contains("nessa_v1."));
    assert!(!logs.contains("recipient.test"));
    assert!(!logs.contains("private-only-fixture"));
    f.reopen();
    let mut retry = f.proposed();
    retry.observed_at_ms = 11_000;
    retry.issue.issued_at = 11;
    let replay = f.command(&f.store).execute(retry).await.unwrap();
    assert_eq!(replay, acknowledgement);
    assert_eq!(f.count("registrations"), 1);
    assert_eq!(f.count("registration_intents"), 1);
}

struct LostInstall<'a> {
    store: &'a SqlOutbox,
    commit: bool,
}
#[async_trait]
impl RegistrationIntents for LostInstall<'_> {
    async fn reserve(&self, intent: RegistrationIntent) -> Result<RegistrationIntent, OutboxError> {
        self.store.reserve(intent).await
    }
    async fn install(&self, intent: RegistrationIntent) -> Result<(), OutboxError> {
        if self.commit {
            RegistrationIntents::install(self.store, intent).await?;
        }
        Err(OutboxError::Unavailable)
    }
    async fn discard(&self, id: Uuid) -> Result<(), OutboxError> {
        self.store.discard(id).await
    }
    async fn cancel_uninstalled(
        &self,
        intent: RegistrationIntent,
        stop: RegistrationStop,
    ) -> Result<crate::webhooks::application::UninstalledCancellation, OutboxError> {
        self.store.cancel_uninstalled(intent, stop).await
    }
}

#[tokio::test]
async fn auth_commit_and_uncertain_install_ack_recover_without_another_credential_or_secret() {
    for committed in [false, true] {
        let mut f = Registered::new();
        let proposal = f.proposed();
        let original = proposal.issue.credential_id.clone();
        assert!(matches!(
            f.command(&LostInstall {
                store: &f.store,
                commit: committed
            })
            .execute(proposal)
            .await,
            Err(RegistrationFailure::Store(OutboxError::Unavailable))
        ));
        assert_eq!(f.count("registrations"), i64::from(committed));
        f.reopen();
        let metadata = f
            .command(&f.store)
            .execute(f.proposed())
            .await
            .unwrap()
            .credential;
        assert_eq!(metadata.id, original);
        assert_eq!(f.count("registrations"), 1);
        assert_eq!(
            f.auth
                .list_sync(&ListCredentialsRequest {
                    organization_id: "org".into()
                })
                .unwrap()
                .len(),
            2
        );
    }
}

#[tokio::test]
async fn conflicting_config_and_caller_inputs_do_not_rewrite_reserved_command() {
    let f = Registered::new();
    let original = f.store.reserve(f.proposed()).await.unwrap();
    for index in 0..5 {
        let mut changed = f.proposed();
        match index {
            0 => changed.config.destination = Destination::parse("https://other.test/").unwrap(),
            1 => changed.config.key = Arc::new(MacKey::new([30; 32])),
            2 => changed.issue.grants.push(grant("conversation.write")),
            3 => changed.issue.expires_at = Some(99),
            4 => changed.issue.principal.id = "other-hook".into(),
            _ => unreachable!(),
        }
        assert!(matches!(
            f.store.reserve(changed).await,
            Err(OutboxError::Conflict)
        ));
    }
    let saved = f.store.reserve(f.proposed()).await.unwrap();
    assert_eq!(saved.id, original.id);
    assert_eq!(saved.issue, original.issue);
    assert_eq!(
        saved.config.key.expose_bytes(),
        original.config.key.expose_bytes()
    );
    assert_eq!(f.count("registration_intents"), 1);
    assert_eq!(f.count("registrations"), 0);
}

#[tokio::test]
async fn unrelated_ordinary_receipt_cannot_install_a_webhook_or_leave_an_intent() {
    let f = Registered::new();
    let mut ordinary = f.proposed().issue;
    ordinary.purpose = CredentialIssuePurpose::Product;
    let _issued = f.auth.issue_sync(ordinary).unwrap();
    assert!(matches!(
        f.command(&f.store).execute(f.proposed()).await,
        Err(RegistrationFailure::Auth(CredentialAdminError::Conflict))
    ));
    assert_eq!(f.count("registration_intents"), 0);
    assert_eq!(f.count("registrations"), 0);
}

#[tokio::test]
async fn replayed_current_revocation_and_expiry_do_not_install_reserved_destination() {
    for revoked in [false, true] {
        let f = Registered::new();
        let intent = f.store.reserve(f.proposed()).await.unwrap();
        let _issued = f.auth.issue_sync(intent.issue.clone()).unwrap();
        if revoked {
            let _revoked = f
                .auth
                .revoke_sync(RevokeCredentialRequest {
                    request_id: "revoke-before-install".into(),
                    issuer_principal_id: "owner".into(),
                    credential_id: intent.issue.credential_id,
                    revoked_at: 11,
                })
                .unwrap();
        } else {
            f.clock.0.store(100_000, Ordering::SeqCst);
        }
        assert!(matches!(
            f.command(&f.store).execute(f.proposed()).await,
            Err(RegistrationFailure::Stopped)
        ));
        assert_eq!(f.count("registrations"), 0);
    }
}

#[tokio::test]
async fn intent_count_and_byte_capacity_refuse_without_partial_insert() {
    let f = Registered::new();
    let mut oversized = f.proposed();
    oversized.issue.grants[0].action = "a".repeat(17 * 1024);
    assert!(matches!(
        f.store.reserve(oversized).await,
        Err(OutboxError::Capacity)
    ));
    assert_eq!(f.count("registration_intents"), 0);
    for index in 0..crate::webhooks::application::MAX_REGISTRATIONS {
        let mut proposed = f.proposed();
        proposed.issue.request_id = format!("capacity-{index}");
        f.store.reserve(proposed).await.unwrap();
    }
    assert!(matches!(
        f.store.reserve(f.proposed()).await,
        Err(OutboxError::Capacity)
    ));
    assert_eq!(f.count("registration_intents"), 64);
}

#[tokio::test]
async fn installed_intent_contradictions_reject_before_live_or_restored_publication() {
    for field in [
        "id",
        "issuer",
        "principal",
        "organization",
        "destination",
        "key",
        "observed_at",
        "issued_at",
        "timestamp_pair",
    ] {
        let mut f = Registered::new();
        let proposed = f.proposed();
        let id = proposed.id;
        let _issued = f.command(&f.store).execute(proposed).await.unwrap();
        let count_before = std::fs::read_dir(&f.audit).unwrap().count();
        {
            let c = f.store.owner.connection.lock().unwrap();
            let trigger: String = c
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE name='registration_intent_immutable'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            c.execute_batch("PRAGMA foreign_keys=OFF; DROP TRIGGER registration_intent_immutable;")
                .unwrap();
            let text: String = c
                .query_row(
                    "SELECT issue FROM registration_intents WHERE id=?1",
                    [id.to_string()],
                    |r| r.get(0),
                )
                .unwrap();
            let mut issue: Value = serde_json::from_str(&text).unwrap();
            match field {
                "id" => {
                    c.execute(
                        "UPDATE registration_intents SET id=?1",
                        [Uuid::new_v4().to_string()],
                    )
                    .unwrap();
                }
                "issuer" => {
                    issue["issuerPrincipalId"] = json!("different-owner");
                    c.execute(
                        "UPDATE registration_intents SET issuer='different-owner'",
                        [],
                    )
                    .unwrap();
                }
                "principal" => issue["principal"]["id"] = json!("different-hook"),
                "organization" => issue["membership"]["organizationId"] = json!("different-org"),
                "destination" => {
                    c.execute(
                        "UPDATE registration_intents SET destination='https://different.test/'",
                        [],
                    )
                    .unwrap();
                }
                "key" => {
                    c.execute(
                        "UPDATE registration_intents SET mac_key=?1",
                        [vec![30u8; 32]],
                    )
                    .unwrap();
                }
                "observed_at" => {
                    c.execute("UPDATE registration_intents SET observed_at=11000", [])
                        .unwrap();
                }
                "issued_at" => issue["issuedAt"] = json!(11),
                "timestamp_pair" => {
                    issue["issuedAt"] = json!(11);
                    c.execute("UPDATE registration_intents SET observed_at=11000", [])
                        .unwrap();
                }
                _ => unreachable!(),
            }
            c.execute(
                "UPDATE registration_intents SET issue=?1",
                [serde_json::to_string(&issue).unwrap()],
            )
            .unwrap();
            c.execute_batch(&trigger).unwrap();
            c.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
            assert_eq!(intents::validate(&c), Err(OutboxError::Invalid), "{field}");
        }
        assert!(
            matches!(f.store.due(20000).await, Err(OutboxError::Invalid)),
            "{field}"
        );
        assert_eq!(std::fs::read_dir(&f.audit).unwrap().count(), count_before);
        // Cold open must also refuse, before publication. Release the original
        // canonical lock using the same isolated replacement helper.
        let replacement = Registered::new();
        drop(std::mem::replace(&mut f.store, replacement.store));
        assert!(
            matches!(
                SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()),
                Err(OutboxError::Invalid)
            ),
            "{field}"
        );
        assert_eq!(std::fs::read_dir(&f.audit).unwrap().count(), count_before);
    }
}

#[tokio::test]
async fn registration_retirement_removes_private_intent_even_when_audit_ack_is_lost() {
    let mut f = Registered::new();
    let proposed = f.proposed();
    let credential = CredentialId::new(&proposed.issue.credential_id).unwrap();
    let _issued = f.command(&f.store).execute(proposed).await.unwrap();
    let canonical = f
        .auth
        .revoke_sync(RevokeCredentialRequest {
            request_id: "retire-synthetic".into(),
            issuer_principal_id: "owner".into(),
            credential_id: credential.as_str().into(),
            revoked_at: 11,
        })
        .unwrap();
    let InitiatorDto::Principal { id: initiator } = canonical.revocation.initiator else {
        panic!("explicit owner")
    };
    let held = f.audit.with_extension("held");
    std::fs::rename(&f.audit, &held).unwrap();
    assert_eq!(
        f.store
            .stop(
                credential,
                StopReason::Revoked {
                    initiator: Initiator::Principal(PrincipalId::new(initiator).unwrap()),
                    cause: RevocationCause::Explicit
                },
                canonical.revocation.at * 1000
            )
            .await,
        Err(OutboxError::Unavailable)
    );
    assert_eq!(f.count("registrations"), 0);
    assert_eq!(f.count("registration_intents"), 0);
    std::fs::rename(held, &f.audit).unwrap();
    f.reopen();
    assert!(f.store.due(20000).await.unwrap().is_empty());
    let logs = std::fs::read_dir(&f.audit)
        .unwrap()
        .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(logs.contains("credential_revoked"));
    assert!(logs.contains("owner"));
    assert!(!logs.contains("recipient.test"));
    f.clock.0.store(12000, Ordering::SeqCst);
    for _ in 0..2 {
        let mut retry = f.proposed();
        retry.observed_at_ms = 12000;
        assert!(matches!(
            f.command(&f.store).execute(retry).await,
            Err(RegistrationFailure::Auth(CredentialAdminError::Conflict))
        ));
        assert_eq!(f.count("registrations"), 0);
        assert_eq!(f.count("registration_intents"), 0);
    }
    let mut next = f.proposed();
    next.observed_at_ms = 12000;
    next.issue.request_id = "new-registration-after-retirement".into();
    assert!(matches!(
        f.command(&f.store).execute(next).await,
        Ok(RegisteredWebhook { .. })
    ));
}

#[tokio::test]
async fn installation_requires_the_retained_original_intent_before_any_effect() {
    let f = Registered::new();
    let original = f.store.reserve(f.proposed()).await.unwrap();
    for field in ["key", "principal", "observation"] {
        let mut changed = original.clone();
        match field {
            "key" => changed.config.key = Arc::new(MacKey::new([31; 32])),
            "principal" => changed.issue.principal.id = "different-hook".into(),
            "observation" => changed.observed_at_ms += 1,
            _ => unreachable!(),
        }
        assert_eq!(
            RegistrationIntents::install(&f.store, changed).await,
            Err(OutboxError::Conflict),
            "{field}"
        );
        assert_eq!(f.count("registrations"), 0);
        assert_eq!(f.count("registration_intents"), 1);
    }
    RegistrationIntents::install(&f.store, original)
        .await
        .unwrap();
    assert_eq!(f.count("registrations"), 1);
}

#[path = "lifecycle.rs"]
mod lifecycle_tests;

#[path = "registration_stop.rs"]
mod stop_tests;

#[path = "registration_cancellation.rs"]
mod cancellation_tests;
