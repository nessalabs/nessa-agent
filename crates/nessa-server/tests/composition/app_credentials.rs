//! An installed app's credential, issued, checked and revoked in a real
//! local registry, as the gateway composes it.
use super::*;
use nessa_auth::adapters::local::BootstrapRequest;
use nessa_auth::application::app_credential::AppCapability;
use nessa_auth::application::dto::{
    CredentialGrantDto, MembershipInputDto, MembershipRoleDto, MembershipStateDto,
    OrganizationInputDto, PrincipalInputDto, PrincipalKindDto, ResourceDto,
};
use nessa_auth::application::ports::CredentialEvidence;
use nessa_auth::application::session::{AuthenticateAppSession, AuthenticateSession};
use nessa_auth::domain::AudienceId;

struct Registry {
    _directory: tempfile::TempDir,
    credentials: LocalAppCredentials,
    store: Arc<LocalCredentialStore>,
    policy: Arc<CedarPolicyEvaluator>,
    owner: AuthenticatedSession,
}

struct Admin(Arc<LocalCredentialStore>);

impl CredentialAdmin for Admin {
    fn issue<'a>(
        &'a self,
        request: nessa_auth::application::credential_admin::IssueCredentialRequest,
    ) -> PortFuture<'a, IssueCredentialOutcome, CredentialAdminError> {
        Box::pin(async move {
            self.0
                .issue_sync(request)
                .map_err(CredentialAdminError::from)
        })
    }
    fn list<'a>(
        &'a self,
        request: ListCredentialsRequest,
    ) -> PortFuture<
        'a,
        Vec<nessa_auth::application::dto::CredentialMetadataDto>,
        CredentialAdminError,
    > {
        Box::pin(async move {
            self.0
                .list_sync(&request)
                .map_err(CredentialAdminError::from)
        })
    }
    fn revoke<'a>(
        &'a self,
        request: RevokeCredentialRequest,
    ) -> PortFuture<
        'a,
        nessa_auth::application::credential_admin::RevokeCredentialOutcome,
        CredentialAdminError,
    > {
        Box::pin(async move {
            self.0
                .revoke_sync(request)
                .map_err(CredentialAdminError::from)
        })
    }
}

fn audience() -> AudienceId {
    AudienceId::new("gateway").unwrap()
}

async fn registry() -> Registry {
    let directory = tempfile::tempdir().unwrap();
    let auth = directory.path().join("auth");
    nessa_local_storage::create_directory(&auth).unwrap();
    let policy = Arc::new(CedarPolicyEvaluator::new().unwrap());
    let store =
        Arc::new(LocalCredentialStore::open(&auth, "credentials.json", policy.clone()).unwrap());
    let bootstrap = store
        .bootstrap(BootstrapRequest {
            gateway_id: "gateway".into(),
            organization: OrganizationInputDto { id: "org".into() },
            principal: PrincipalInputDto {
                id: "owner".into(),
                kind: PrincipalKindDto::Human,
            },
            membership: MembershipInputDto {
                id: "member".into(),
                principal_id: "owner".into(),
                organization_id: "org".into(),
                role: MembershipRoleDto::Admin,
                state: MembershipStateDto::Active,
            },
            credential_id: "credential".into(),
            issued_at: SystemClock.unix_seconds() - 1,
            expires_at: None,
            grants: vec![CredentialGrantDto {
                action: "credential.manage".into(),
                resource: ResourceDto {
                    organization_id: "org".into(),
                    id: "gateway".into(),
                },
            }],
        })
        .unwrap();
    let owner = AuthenticateSession {
        policy: policy.as_ref(),
        verifier: store.as_ref(),
        access: store.as_ref(),
        clock: &SystemClock,
    }
    .execute(&bootstrap.evidence, &audience())
    .await
    .unwrap();
    Registry {
        _directory: directory,
        credentials: LocalAppCredentials {
            store: store.clone(),
            admin: Arc::new(Admin(store.clone())),
            policy: policy.clone(),
        },
        store,
        policy,
        owner,
    }
}

fn app(id: &str) -> ExtensionAppId {
    ExtensionAppId::new(id).unwrap()
}

impl Registry {
    async fn issue(&self, request: &str, id: &str) -> IssuedAppCredential {
        self.credentials
            .issue(
                &self.owner,
                request.into(),
                app(id),
                AppCapabilities::new(AppCapability::List, &[AppCapability::Create]),
            )
            .await
            .unwrap_or_else(|refusal| panic!("issue refused: {refusal:?}"))
    }

    /// The app `secret` signs in as, or why it cannot.
    async fn sign_in(
        &self,
        secret: &str,
    ) -> Result<String, nessa_auth::application::ports::AccessError> {
        let evidence = CredentialEvidence::new(secret.as_bytes().to_vec()).unwrap();
        AuthenticateAppSession {
            credentials: self.credentials.sessions(),
            clock: &SystemClock,
            policy: self.policy.as_ref(),
        }
        .execute(&evidence, &audience())
        .await
        .map(|session| session.binding().app().as_str().to_owned())
    }
}

#[tokio::test]
async fn an_issued_credential_signs_in_as_its_app_with_exactly_its_capabilities() {
    let registry = registry().await;
    let issued = registry.issue("r1", "notes").await;
    assert_eq!(
        issued.capabilities,
        vec![AppCapability::List, AppCapability::Create]
    );
    assert!(issued.expires_at > SystemClock.unix_seconds() + 300 * 24 * 60 * 60);
    assert_eq!(registry.sign_in(&issued.secret).await.unwrap(), "notes");
    // The ordinary verifier never takes an app's credential as a person's.
    let evidence = CredentialEvidence::new(issued.secret.as_bytes().to_vec()).unwrap();
    assert!(AuthenticateSession {
        policy: registry.policy.as_ref(),
        verifier: registry.store.as_ref(),
        access: registry.store.as_ref(),
        clock: &SystemClock,
    }
    .execute(&evidence, &audience())
    .await
    .is_err());
}

#[tokio::test]
async fn issuing_again_revokes_the_one_before_and_a_replay_is_a_conflict() {
    let registry = registry().await;
    let first = registry.issue("r1", "notes").await;
    let second = registry.issue("r2", "notes").await;
    assert!(registry.sign_in(&first.secret).await.is_err());
    assert_eq!(registry.sign_in(&second.secret).await.unwrap(), "notes");
    let replay = registry
        .credentials
        .issue(
            &registry.owner,
            "r2".into(),
            app("notes"),
            AppCapabilities::new(AppCapability::List, &[]),
        )
        .await;
    assert!(matches!(replay, Err(CredentialRefusal::Conflict)));
    // A refused replay leaves the app's working credential alone.
    assert_eq!(registry.sign_in(&second.secret).await.unwrap(), "notes");
}

#[tokio::test]
async fn a_late_retry_of_a_revoke_never_reaches_a_credential_issued_after_it() {
    let registry = registry().await;
    registry.issue("r1", "notes").await;
    let revoke = |request: &'static str| {
        registry
            .credentials
            .revoke_all("org".into(), "owner".into(), request.into(), app("notes"))
    };
    assert_eq!(revoke("r2").await.unwrap(), 1);
    let fresh = registry.issue("r3", "notes").await;
    // The retry is refused, and the credential issued after it still works.
    assert!(matches!(
        revoke("r2").await,
        Err(CredentialRefusal::Conflict)
    ));
    assert_eq!(registry.sign_in(&fresh.secret).await.unwrap(), "notes");
}

#[tokio::test]
async fn revoking_an_app_ends_only_its_own_credentials() {
    let registry = registry().await;
    let notes = registry.issue("r1", "notes").await;
    let calendar = registry.issue("r2", "calendar").await;
    let revoked = registry
        .credentials
        .revoke_all("org".into(), "owner".into(), "r3".into(), app("notes"))
        .await
        .unwrap();
    assert_eq!(revoked, 1);
    assert!(registry.sign_in(&notes.secret).await.is_err());
    assert_eq!(
        registry.sign_in(&calendar.secret).await.unwrap(),
        "calendar"
    );
    // A retry of the same command answers what it revoked, so its caller
    // can still record it.
    let retried = registry
        .credentials
        .revoke_all("org".into(), "owner".into(), "r3".into(), app("notes"))
        .await
        .unwrap();
    assert_eq!(retried, 1);
    // Nothing left to revoke is not a failure.
    let again = registry
        .credentials
        .revoke_all("org".into(), "owner".into(), "r4".into(), app("notes"))
        .await
        .unwrap();
    assert_eq!(again, 0);
}
