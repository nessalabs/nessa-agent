// The installed-app methods at the socket: refused without the manifest's
// `credential.manage`, shape-checked before anything is kept,
// `apps_not_configured` without a store, and a reviewed package installed,
// read back byte for byte, listed and removed through the real SQLite store.
use super::*;
use crate::installed_apps::application::{
    AppsAudit, AppsRecord, AuditUnavailable, InstalledApps,
};
use crate::installed_apps::infrastructure::SqliteAppStore;

/// The apps audit; refuses every record while `.1` is set.
#[derive(Default)]
struct Kept(Mutex<Vec<AppsRecord>>, std::sync::atomic::AtomicBool);

impl AppsAudit for Kept {
    fn record(&self, record: &AppsRecord) -> Result<(), AuditUnavailable> {
        if self.1.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(AuditUnavailable);
        }
        self.0.lock().unwrap().push(record.clone());
        Ok(())
    }
}

fn apps() -> (tempfile::TempDir, Arc<InstalledApps>, Arc<Kept>) {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().join("private");
    nessa_local_storage::create_directory(&root).unwrap();
    let store = SqliteAppStore::open(&root.join("apps.sqlite3")).unwrap();
    let kept = Arc::new(Kept::default());
    (
        folder,
        Arc::new(InstalledApps::new(Box::new(store), kept.clone())),
        kept,
    )
}

fn with(id: &str, method: &str, params: Value) -> RequestFrame {
    let mut frame = request(id, method);
    frame.params = params;
    frame
}

fn payload(message: OutgoingMessage) -> Value {
    let OutgoingMessage::Response(response) = message else {
        panic!("response expected");
    };
    assert!(response.ok, "{:?}", response.error);
    response.payload.unwrap()
}

async fn owner(grants: &[&str], role: MembershipRole) -> (ProductRouteState, AuthenticatedSession, Arc<Kept>, tempfile::TempDir) {
    let (folder, apps, kept) = apps();
    let (state, authority) = fixture(role);
    *authority.snapshot.lock().unwrap() =
        snapshot_with(role, MembershipStatus::Active, grants);
    let state = state.with_apps(apps);
    let session = authenticate(&state).await;
    (state, session, kept, folder)
}

#[test]
fn every_apps_method_is_admitted_as_credential_manage() {
    for method in [
        "apps.starter",
        "apps.review",
        "apps.install",
        "apps.installed",
        "apps.read",
        "apps.uninstall",
    ] {
        assert_eq!(
            nessa_protocol::product::generated::action_for_method(method),
            Some("credential.manage"),
            "{method}"
        );
    }
}

#[tokio::test]
async fn without_a_store_every_method_says_it_is_not_configured() {
    let (state, authority) = fixture(MembershipRole::Admin);
    *authority.snapshot.lock().unwrap() = snapshot_with(
        MembershipRole::Admin,
        MembershipStatus::Active,
        &["credential.manage"],
    );
    let session = authenticate(&state).await;
    assert_eq!(
        code_of(dispatch(&state, &session, request("1", "apps.installed")).await),
        "apps_not_configured"
    );
}

#[tokio::test]
async fn a_caller_without_credential_manage_installs_nothing() {
    let (state, session, kept, _folder) =
        owner(&["conversation.write"], MembershipRole::Member).await;
    let starter = with(
        "1",
        "apps.starter",
        json!({ "appId": "notes", "packageVersion": "1.0.0" }),
    );
    assert_eq!(code_of(dispatch(&state, &session, starter).await), "forbidden");
    let install = with(
        "2",
        "apps.install",
        json!({
            "requestId": "r1",
            "package": { "manifest": "e30=", "assets": [] },
            "reviewedDigest": "a".repeat(64),
        }),
    );
    assert_eq!(code_of(dispatch(&state, &session, install).await), "forbidden");
    assert!(kept.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_owner_installs_the_reviewed_package_reads_it_back_and_removes_it() {
    let (state, session, kept, _folder) =
        owner(&["credential.manage"], MembershipRole::Admin).await;
    let made = payload(
        dispatch(
            &state,
            &session,
            with(
                "1",
                "apps.starter",
                json!({ "appId": "notes", "packageVersion": "1.0.0" }),
            ),
        )
        .await,
    );
    let package = made["package"].clone();
    let digest = made["review"]["digest"].as_str().unwrap().to_owned();
    let reviewed = payload(
        dispatch(
            &state,
            &session,
            with("2", "apps.review", json!({ "package": package })),
        )
        .await,
    );
    assert_eq!(reviewed, made["review"]);

    // A digest the owner did not review installs nothing.
    let stale = with(
        "3",
        "apps.install",
        json!({ "requestId": "r0", "package": package, "reviewedDigest": "0".repeat(64) }),
    );
    assert_eq!(
        code_of(dispatch(&state, &session, stale).await),
        "app_review_stale"
    );
    assert!(kept.0.lock().unwrap().is_empty());

    let installed = payload(
        dispatch(
            &state,
            &session,
            with(
                "4",
                "apps.install",
                json!({ "requestId": "r1", "package": package, "reviewedDigest": digest }),
            ),
        )
        .await,
    );
    assert_eq!(
        installed,
        json!({ "requestId": "r1", "app": { "appId": "notes", "packageVersion": "1.0.0" } })
    );
    assert_eq!(
        payload(dispatch(&state, &session, request("5", "apps.installed")).await),
        json!({ "apps": [{ "appId": "notes", "packageVersion": "1.0.0" }] })
    );
    let read = payload(
        dispatch(
            &state,
            &session,
            with("6", "apps.read", json!({ "appId": "notes" })),
        )
        .await,
    );
    assert_eq!(read["package"], package, "the bytes read are the bytes reviewed");
    assert_eq!(read["review"]["digest"], digest.as_str());

    let removed = payload(
        dispatch(
            &state,
            &session,
            with(
                "7",
                "apps.uninstall",
                json!({ "requestId": "u1", "appId": "notes" }),
            ),
        )
        .await,
    );
    assert_eq!(removed, json!({ "requestId": "u1", "appId": "notes" }));
    assert_eq!(
        payload(dispatch(&state, &session, request("8", "apps.installed")).await),
        json!({ "apps": [] })
    );
    assert_eq!(kept.0.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn shapes_are_checked_before_anything_is_kept() {
    let (state, session, kept, _folder) =
        owner(&["credential.manage"], MembershipRole::Admin).await;
    for (id, method, params) in [
        ("1", "apps.starter", json!({ "appId": "nessa", "packageVersion": "1" })),
        ("2", "apps.starter", json!({ "appId": "notes" })),
        ("3", "apps.read", json!({ "appId": "Not An Id" })),
        ("4", "apps.uninstall", json!({ "appId": "notes" })),
        ("5", "apps.installed", json!({ "extra": true })),
        (
            "6",
            "apps.install",
            json!({ "requestId": "r", "package": { "manifest": "!!", "assets": [] }, "reviewedDigest": "a".repeat(64) }),
        ),
    ] {
        let code = code_of(dispatch(&state, &session, with(id, method, params)).await);
        assert!(
            ["invalid_request", "app_package_invalid"].contains(&code.as_str()),
            "{method}: {code}"
        );
    }
    assert!(kept.0.lock().unwrap().is_empty());
}

mod credentials {
    use super::*;
    use crate::installed_apps::application::{
        AppCredentialAuthority, AppsEvent, CredentialRefusal, IssuedAppCredential,
    };
    use nessa_auth::application::app_credential::{
        AppCapabilities, AppCapability, AppCredentialSnapshot, AppCredentials,
    };
    use nessa_auth::application::ports::{AccessError, CredentialEvidence, PortFuture};
    use nessa_auth::domain::{AudienceId, CredentialId};
    use nessa_types::app_identity::ExtensionAppId;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A credential store that records what it was asked.
    #[derive(Default)]
    struct Store {
        issued: Mutex<Vec<(String, Vec<AppCapability>)>>,
        revoked: Mutex<Vec<String>>,
        revoke_fails: AtomicBool,
    }

    impl AppCredentials for Store {
        fn verify_app<'a>(
            &'a self,
            _: &'a CredentialEvidence,
            _: &'a AudienceId,
        ) -> PortFuture<'a, AppCredentialSnapshot> {
            Box::pin(async { Err(AccessError::InvalidCredential) })
        }
        fn read_app<'a>(&'a self, _: &'a CredentialId) -> PortFuture<'a, AppCredentialSnapshot> {
            Box::pin(async { Err(AccessError::InvalidCredential) })
        }
    }

    impl AppCredentialAuthority for Store {
        fn issue<'a>(
            &'a self,
            _: &'a AuthenticatedSession,
            _: String,
            app: ExtensionAppId,
            capabilities: AppCapabilities,
        ) -> PortFuture<'a, IssuedAppCredential, CredentialRefusal> {
            let capabilities: Vec<_> = capabilities.iter().collect();
            self.issued
                .lock()
                .unwrap()
                .push((app.as_str().to_owned(), capabilities.clone()));
            Box::pin(async move {
                Ok(IssuedAppCredential {
                    credential_id: "app-1".into(),
                    secret: "app-1.secret".into(),
                    expires_at: 99,
                    capabilities,
                })
            })
        }
        fn revoke_all<'a>(
            &'a self,
            _: String,
            _: String,
            _: String,
            app: ExtensionAppId,
        ) -> PortFuture<'a, usize, CredentialRefusal> {
            let fails = self.revoke_fails.load(Ordering::SeqCst);
            if !fails {
                self.revoked.lock().unwrap().push(app.as_str().to_owned());
            }
            Box::pin(async move {
                if fails {
                    Err(CredentialRefusal::Unavailable)
                } else {
                    Ok(1)
                }
            })
        }
        fn sessions(&self) -> &dyn AppCredentials {
            self
        }
    }

    struct Fixture {
        state: ProductRouteState,
        session: AuthenticatedSession,
        kept: Arc<Kept>,
        store: Arc<Store>,
        _folder: tempfile::TempDir,
    }

    async fn fixture() -> Fixture {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("private");
        nessa_local_storage::create_directory(&root).unwrap();
        let kept = Arc::new(Kept::default());
        let store = Arc::new(Store::default());
        let apps = InstalledApps::new(
            Box::new(SqliteAppStore::open(&root.join("apps.sqlite3")).unwrap()),
            kept.clone(),
        )
        .with_credentials(store.clone());
        let (state, authority) = super::fixture(MembershipRole::Admin);
        *authority.snapshot.lock().unwrap() = snapshot_with(
            MembershipRole::Admin,
            MembershipStatus::Active,
            &["credential.manage"],
        );
        let state = state.with_apps(Arc::new(apps));
        let session = authenticate(&state).await;
        Fixture {
            state,
            session,
            kept,
            store,
            _folder: folder,
        }
    }

    impl Fixture {
        async fn call(&self, method: &str, params: Value) -> OutgoingMessage {
            dispatch(&self.state, &self.session, with("1", method, params)).await
        }

        /// Install the starter for `app`, its manifest asking `requested`.
        async fn install(&self, app: &str, requested: &[&str]) -> Value {
            let made = payload(
                self.call(
                    "apps.starter",
                    json!({ "appId": app, "packageVersion": "1.0.0" }),
                )
                .await,
            );
            let engine = base64::engine::general_purpose::STANDARD;
            let mut manifest: Value = serde_json::from_slice(
                &engine
                    .decode(made["package"]["manifest"].as_str().unwrap())
                    .unwrap(),
            )
            .unwrap();
            manifest["requestedCapabilities"] = json!(requested);
            let mut package = made["package"].clone();
            package["manifest"] = json!(engine.encode(serde_json::to_vec(&manifest).unwrap()));
            let review = payload(self.call("apps.review", json!({ "package": package })).await);
            payload(
                self.call(
                    "apps.install",
                    json!({ "requestId": format!("i-{app}"), "package": package, "reviewedDigest": review["digest"] }),
                )
                .await,
            );
            review
        }
    }

    #[test]
    fn the_credential_methods_are_admitted_as_credential_manage() {
        for method in ["apps.issueCredential", "apps.revokeCredentials"] {
            assert_eq!(
                nessa_protocol::product::generated::action_for_method(method),
                Some("credential.manage"),
                "{method}"
            );
        }
    }

    #[tokio::test]
    async fn an_installed_app_is_issued_exactly_what_its_package_requests() {
        let fixture = fixture().await;
        let review = fixture.install("notes", &["send", "list", "create"]).await;
        // The review names them in a fixed order, as the owner approves them.
        assert_eq!(
            review["requestedCapabilities"],
            json!(["list", "create", "send"])
        );
        let issued = payload(
            fixture
                .call(
                    "apps.issueCredential",
                    json!({ "requestId": "c1", "appId": "notes" }),
                )
                .await,
        );
        assert_eq!(
            issued,
            json!({
                "requestId": "c1",
                "appId": "notes",
                "credentialId": "app-1",
                "credential": "app-1.secret",
                "expiresAt": 99,
                "capabilities": ["list", "create", "send"],
            })
        );
        assert_eq!(
            *fixture.store.issued.lock().unwrap(),
            vec![(
                "notes".to_owned(),
                vec![AppCapability::List, AppCapability::Create, AppCapability::Send]
            )]
        );
        assert!(fixture.kept.0.lock().unwrap().iter().any(|record| matches!(
            &record.event,
            AppsEvent::CredentialIssued { credential_id, .. } if credential_id == "app-1"
        )));
    }

    #[tokio::test]
    async fn a_credential_whose_record_is_lost_is_taken_back_and_reported() {
        let fixture = fixture().await;
        fixture.install("notes", &["list"]).await;
        fixture.kept.1.store(true, Ordering::SeqCst);
        assert_eq!(
            code_of(
                fixture
                    .call(
                        "apps.issueCredential",
                        json!({ "requestId": "c1", "appId": "notes" }),
                    )
                    .await
            ),
            "apps_audit_unavailable"
        );
        // Issued, then revoked again: nobody holds a credential with no record.
        assert_eq!(fixture.store.issued.lock().unwrap().len(), 1);
        assert_eq!(*fixture.store.revoked.lock().unwrap(), vec!["notes"]);
        // A revoke whose record is lost is reported too; it still revoked.
        assert_eq!(
            code_of(
                fixture
                    .call(
                        "apps.revokeCredentials",
                        json!({ "requestId": "r1", "appId": "notes" }),
                    )
                    .await
            ),
            "apps_audit_unavailable"
        );
        assert_eq!(fixture.store.revoked.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn an_app_not_installed_or_asking_nothing_is_issued_nothing() {
        let fixture = fixture().await;
        assert_eq!(
            code_of(
                fixture
                    .call(
                        "apps.issueCredential",
                        json!({ "requestId": "c1", "appId": "notes" }),
                    )
                    .await
            ),
            "app_not_installed"
        );
        fixture.install("notes", &[]).await;
        assert_eq!(
            code_of(
                fixture
                    .call(
                        "apps.issueCredential",
                        json!({ "requestId": "c2", "appId": "notes" }),
                    )
                    .await
            ),
            "app_requests_nothing"
        );
        assert!(fixture.store.issued.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn uninstalling_revokes_first_and_a_failed_revoke_keeps_the_app() {
        let fixture = fixture().await;
        fixture.install("notes", &["list"]).await;
        fixture.store.revoke_fails.store(true, Ordering::SeqCst);
        assert_eq!(
            code_of(
                fixture
                    .call(
                        "apps.uninstall",
                        json!({ "requestId": "u1", "appId": "notes" }),
                    )
                    .await
            ),
            "app_credentials_unavailable"
        );
        assert_eq!(
            payload(fixture.call("apps.installed", json!({})).await),
            json!({ "apps": [{ "appId": "notes", "packageVersion": "1.0.0" }] }),
            "an app that could still sign in is not removed"
        );
        fixture.store.revoke_fails.store(false, Ordering::SeqCst);
        payload(
            fixture
                .call(
                    "apps.uninstall",
                    json!({ "requestId": "u2", "appId": "notes" }),
                )
                .await,
        );
        assert_eq!(*fixture.store.revoked.lock().unwrap(), vec!["notes"]);
        assert_eq!(
            payload(
                fixture
                    .call(
                        "apps.revokeCredentials",
                        json!({ "requestId": "r1", "appId": "calendar" }),
                    )
                    .await
            ),
            json!({ "requestId": "r1", "appId": "calendar", "revoked": 1 })
        );
    }
}
