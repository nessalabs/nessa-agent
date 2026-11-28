//! What each apps operation leaves in the store and the audit, against a real
//! SQLite store in a temporary folder and an audit that keeps its records.
use std::sync::{Arc, Mutex};

use base64::{engine::general_purpose::STANDARD, Engine};
use nessa_app_package::{InlineAsset, InlinePackage};
use nessa_types::app_identity::ExtensionAppId;
use sha2::{Digest, Sha256};

use super::application::{
    AppsAudit, AppsCaller, AppsError, AppsEvent, AppsRecord, AuditUnavailable, InstalledApps,
};
use super::infrastructure::SqliteAppStore;

#[derive(Default)]
struct Kept {
    records: Mutex<Vec<AppsRecord>>,
    /// Refuse every record from this many onwards.
    refuse_from: Mutex<Option<usize>>,
}

impl AppsAudit for Kept {
    fn record(&self, record: &AppsRecord) -> Result<(), AuditUnavailable> {
        let mut records = self.records.lock().unwrap();
        if self
            .refuse_from
            .lock()
            .unwrap()
            .is_some_and(|from| records.len() >= from)
        {
            return Err(AuditUnavailable);
        }
        records.push(record.clone());
        Ok(())
    }
}

fn owner() -> AppsCaller {
    AppsCaller {
        organization_id: "org".into(),
        principal_id: "owner".into(),
        credential_id: "desktop".into(),
    }
}

fn apps() -> (tempfile::TempDir, InstalledApps, Arc<Kept>) {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().join("private");
    nessa_local_storage::create_directory(&root).unwrap();
    let store = SqliteAppStore::open(&root.join("apps.sqlite3")).unwrap();
    let audit = Arc::new(Kept::default());
    let apps = InstalledApps::new(Box::new(store), audit.clone());
    (folder, apps, audit)
}

fn events(audit: &Kept) -> Vec<AppsEvent> {
    audit
        .records
        .lock()
        .unwrap()
        .iter()
        .map(|record| record.event.clone())
        .collect()
}

fn id(text: &str) -> ExtensionAppId {
    ExtensionAppId::new(text).unwrap()
}

/// The same app with its HTML changed and its manifest updated to match: a
/// package that verifies, but is not the one reviewed.
/// `package` with its `index.html` replaced by `html`, declared to match.
fn edited(package: &InlinePackage, html: &[u8]) -> InlinePackage {
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&STANDARD.decode(&package.manifest_base64).unwrap()).unwrap();
    let declared = manifest["assets"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|asset| asset["path"] == "index.html")
        .unwrap();
    declared["sizeBytes"] = html.len().into();
    declared["sha256"] = hex::encode(Sha256::digest(html)).into();
    InlinePackage {
        manifest_base64: STANDARD.encode(serde_json::to_vec(&manifest).unwrap()),
        assets: package
            .assets
            .iter()
            .map(|asset| InlineAsset {
                path: asset.path.clone(),
                bytes_base64: if asset.path == "index.html" {
                    STANDARD.encode(html)
                } else {
                    asset.bytes_base64.clone()
                },
            })
            .collect(),
    }
}

#[tokio::test]
async fn the_reviewed_bytes_install_and_read_back_exactly() {
    let (_folder, apps, audit) = apps();
    let starter = apps.starter("notes".into(), "1.0.0".into(), None).unwrap();
    assert_eq!(starter.review.app_id, "notes");
    assert_eq!(starter.review.entrypoint, "index.html");
    assert_eq!(
        apps.review(&starter.package).unwrap(),
        starter.review,
        "reviewing the starter finds what the starter said"
    );
    assert!(apps.installed().await.unwrap().is_empty());

    let installed = apps
        .install(
            owner(),
            "r1".into(),
            &starter.package,
            &starter.review.digest,
        )
        .await
        .unwrap();
    assert_eq!(installed.app_id, "notes");
    let read = apps.read(id("notes")).await.unwrap();
    assert_eq!(
        read.package, starter.package,
        "the installed bytes are the reviewed bytes"
    );
    assert_eq!(read.review.digest, starter.review.digest);
    assert_eq!(
        events(&audit),
        vec![
            AppsEvent::InstallRequested {
                digest: starter.review.digest.clone(),
                package_version: "1.0.0".into(),
            },
            AppsEvent::Installed {
                digest: starter.review.digest.clone(),
            },
        ]
    );
    let first = audit.records.lock().unwrap()[0].clone();
    assert_eq!(first.request_id, "r1");
    assert_eq!(first.app_id, "notes");
    assert_eq!(first.caller, owner());

    // A retry of the same install answers the same app, recording that it
    // is installed, and changes nothing else.
    assert_eq!(
        apps.install(
            owner(),
            "r1".into(),
            &starter.package,
            &starter.review.digest
        )
        .await
        .unwrap(),
        installed
    );
    assert_eq!(
        events(&audit)[2..],
        [AppsEvent::Installed {
            digest: starter.review.digest.clone(),
        }]
    );
    assert_eq!(apps.installed().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_package_that_is_not_the_reviewed_one_writes_nothing() {
    let (_folder, apps, audit) = apps();
    let starter = apps.starter("notes".into(), "1.0.0".into(), None).unwrap();
    let swapped = edited(&starter.package, b"<!doctype html><p>not what you saw</p>");
    assert_ne!(apps.review(&swapped).unwrap().digest, starter.review.digest);
    assert_eq!(
        apps.install(owner(), "r1".into(), &swapped, &starter.review.digest)
            .await,
        Err(AppsError::ReviewStale)
    );
    // Bytes that do not match their own manifest are refused too.
    let mut tampered = starter.package.clone();
    tampered.assets[0].bytes_base64 = STANDARD.encode(b"<p>tampered</p>");
    assert_eq!(
        apps.install(owner(), "r2".into(), &tampered, &starter.review.digest)
            .await,
        Err(AppsError::PackageInvalid)
    );
    assert!(apps.installed().await.unwrap().is_empty());
    assert!(events(&audit).is_empty(), "nothing was asked of the store");
}

#[tokio::test]
async fn another_package_under_an_installed_identity_is_refused() {
    let (_folder, apps, audit) = apps();
    let starter = apps.starter("notes".into(), "1.0.0".into(), None).unwrap();
    apps.install(
        owner(),
        "r1".into(),
        &starter.package,
        &starter.review.digest,
    )
    .await
    .unwrap();
    let other = edited(&starter.package, b"<!doctype html><p>v2</p>");
    let digest = apps.review(&other).unwrap().digest;
    assert_eq!(
        apps.install(owner(), "r2".into(), &other, &digest).await,
        Err(AppsError::AlreadyInstalled)
    );
    assert_eq!(
        apps.read(id("notes")).await.unwrap().package,
        starter.package
    );
    assert_eq!(events(&audit).len(), 2);
}

#[tokio::test]
async fn uninstall_removes_one_app_and_leaves_the_other() {
    let (_folder, apps, audit) = apps();
    for name in ["calendar", "notes"] {
        let starter = apps.starter(name.into(), "1.0.0".into(), None).unwrap();
        apps.install(
            owner(),
            name.into(),
            &starter.package,
            &starter.review.digest,
        )
        .await
        .unwrap();
    }
    let notes = apps.read(id("notes")).await.unwrap();
    assert_eq!(
        apps.uninstall(owner(), "u1".into(), id("calendar")).await,
        Ok("calendar".into())
    );
    let listed = apps.installed().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].app_id, "notes");
    assert_eq!(apps.read(id("notes")).await.unwrap().package, notes.package);
    assert_eq!(
        apps.read(id("calendar")).await.err(),
        Some(AppsError::NotInstalled)
    );
    assert_eq!(
        events(&audit)[4..],
        [
            AppsEvent::UninstallRequested {
                package_version: "1.0.0".into()
            },
            AppsEvent::Uninstalled,
        ]
    );
    assert_eq!(
        apps.uninstall(owner(), "u2".into(), id("calendar")).await,
        Err(AppsError::NotInstalled)
    );
    assert_eq!(events(&audit).len(), 6);
}

#[tokio::test]
async fn a_change_that_cannot_be_recorded_is_not_made_or_not_answered_as_made() {
    let (_folder, apps, audit) = apps();
    let starter = apps.starter("notes".into(), "1.0.0".into(), None).unwrap();
    // The request cannot be recorded: nothing is installed.
    *audit.refuse_from.lock().unwrap() = Some(0);
    assert_eq!(
        apps.install(
            owner(),
            "r1".into(),
            &starter.package,
            &starter.review.digest
        )
        .await,
        Err(AppsError::AuditUnavailable)
    );
    assert!(apps.installed().await.unwrap().is_empty());
    // The outcome cannot be recorded: the caller is told, and the app stays.
    *audit.refuse_from.lock().unwrap() = Some(1);
    assert_eq!(
        apps.install(
            owner(),
            "r2".into(),
            &starter.package,
            &starter.review.digest
        )
        .await,
        Err(AppsError::AuditUnavailable)
    );
    assert_eq!(apps.installed().await.unwrap().len(), 1);
    // A retry while the audit still refuses is not answered as a success.
    assert_eq!(
        apps.install(
            owner(),
            "r2".into(),
            &starter.package,
            &starter.review.digest
        )
        .await,
        Err(AppsError::AuditUnavailable)
    );
    // Once the audit keeps records again, the retry records the outcome.
    *audit.refuse_from.lock().unwrap() = None;
    apps.install(
        owner(),
        "r2".into(),
        &starter.package,
        &starter.review.digest,
    )
    .await
    .unwrap();
    assert_eq!(
        events(&audit).last(),
        Some(&AppsEvent::Installed {
            digest: starter.review.digest.clone(),
        })
    );
}

#[tokio::test]
async fn an_uninstall_and_a_reinstall_at_once_never_remove_what_was_not_named() {
    let (_folder, apps, audit) = apps();
    let apps = Arc::new(apps);
    let one = apps.starter("notes".into(), "1.0.0".into(), None).unwrap();
    let two = apps.starter("notes".into(), "2.0.0".into(), None).unwrap();
    apps.install(owner(), "r1".into(), &one.package, &one.review.digest)
        .await
        .unwrap();
    // An uninstall, and an uninstall then reinstall of version 2, at once.
    let first = {
        let apps = apps.clone();
        tokio::spawn(async move { apps.uninstall(owner(), "u1".into(), id("notes")).await })
    };
    let second = {
        let apps = apps.clone();
        tokio::spawn(async move {
            let _ = apps.uninstall(owner(), "u2".into(), id("notes")).await;
            apps.install(owner(), "r2".into(), &two.package, &two.review.digest)
                .await
        })
    };
    let _ = first.await.unwrap();
    let _ = second.await.unwrap();
    // Every removal names the version that was there when it began, and
    // whatever is left is what the last change left.
    let records = events(&audit);
    let mut there: Option<String> = Some("1.0.0".into());
    for event in &records {
        match event {
            AppsEvent::UninstallRequested { package_version } => {
                assert_eq!(Some(package_version), there.as_ref(), "{records:?}");
            }
            AppsEvent::Uninstalled => there = None,
            AppsEvent::InstallRequested {
                package_version, ..
            } => {
                there = Some(package_version.clone());
            }
            _ => {}
        }
    }
    let left = apps.installed().await.unwrap();
    assert_eq!(
        left.first().map(|app| app.package_version.clone()),
        there,
        "{records:?}"
    );
}

#[tokio::test]
async fn a_package_outside_the_inline_limits_is_too_large() {
    let (_folder, apps, _audit) = apps();
    let starter = apps.starter("notes".into(), "1.0.0".into(), None).unwrap();
    let big = edited(&starter.package, &vec![b'a'; 17 * 1024]);
    assert_eq!(apps.review(&big), Err(AppsError::PackageTooLarge));
}

#[tokio::test]
async fn no_more_apps_install_than_the_list_can_hold() {
    let (_folder, apps, audit) = apps();
    for index in 0..super::application::MAX_INSTALLED_APPS {
        let starter = apps
            .starter(format!("app-{index}"), "1.0.0".into(), None)
            .unwrap();
        apps.install(
            owner(),
            format!("r{index}"),
            &starter.package,
            &starter.review.digest,
        )
        .await
        .unwrap();
    }
    let recorded = events(&audit).len();
    let one_more = apps.starter("one-more".into(), "1.0.0".into(), None).unwrap();
    assert_eq!(
        apps.install(
            owner(),
            "past".into(),
            &one_more.package,
            &one_more.review.digest
        )
        .await,
        Err(AppsError::LimitReached)
    );
    assert_eq!(
        events(&audit).len(),
        recorded,
        "nothing was asked of the store"
    );
    assert_eq!(
        apps.installed().await.unwrap().len(),
        super::application::MAX_INSTALLED_APPS
    );
    // A retry of an installed app's own bytes is still answered.
    let first = apps.starter("app-0".into(), "1.0.0".into(), None).unwrap();
    assert!(apps
        .install(owner(), "r0".into(), &first.package, &first.review.digest)
        .await
        .is_ok());
}
