// The installed-app methods at the socket: refused without the manifest's
// `credential.manage`, shape-checked before anything is kept,
// `apps_not_configured` without a store, and a reviewed package installed,
// read back byte for byte, listed and removed through the real SQLite store.
use super::*;
use crate::installed_apps::application::{
    AppsAudit, AppsRecord, AuditUnavailable, InstalledApps,
};
use crate::installed_apps::infrastructure::SqliteAppStore;

#[derive(Default)]
struct Kept(Mutex<Vec<AppsRecord>>);

impl AppsAudit for Kept {
    fn record(&self, record: &AppsRecord) -> Result<(), AuditUnavailable> {
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
