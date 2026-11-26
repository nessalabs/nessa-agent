//! The two fixture packages and private SQLite setup.

use std::{collections::BTreeMap, path::Path};

use nessa_app_package::{verify_package, InstalledAppStore, VerifiedPackage};
use serde_json::json;
use sha2::{Digest, Sha256};

pub const CALENDAR: &str = "fixture-calendar";
pub const NOTES: &str = "fixture-notes";
const ENTRYPOINT: &str = "index.html";
const STYLESHEET: &str = "static/style.css";

pub fn install(database: &Path) -> Result<[VerifiedPackage; 2], String> {
    let calendar = package(CALENDAR, "Calendar from installed bytes", 137)?;
    let notes = package(NOTES, "Notes from installed bytes", 173)?;
    {
        let mut store = open(database)?;
        for package in [&calendar, &notes] {
            store.install(package).map_err(|error| {
                format!(
                    "install {}: {error:?}",
                    package.manifest().app_id().as_str()
                )
            })?;
        }
    }
    let mut store = open(database)?;
    Ok([load(&mut store, &calendar)?, load(&mut store, &notes)?])
}

pub fn uninstall(
    database: &Path,
    target: &VerifiedPackage,
    sibling: &VerifiedPackage,
) -> Result<(), String> {
    let app_id = target.manifest().app_id();
    {
        let mut store = open(database)?;
        // Stop should leave the installed record intact for explicit removal.
        load(&mut store, target)?;
        store
            .remove(app_id)
            .map_err(|error| format!("uninstall {}: {error:?}", app_id.as_str()))?;
    }
    let mut store = open(database)?;
    if store
        .load(app_id)
        .map_err(|error| format!("check uninstalled {}: {error:?}", app_id.as_str()))?
        .is_some()
    {
        return Err(format!("{} is still installed", app_id.as_str()));
    }
    load(&mut store, sibling)?;
    Ok(())
}

fn open(database: &Path) -> Result<InstalledAppStore, String> {
    InstalledAppStore::open(database).map_err(|error| format!("open fixture store: {error:?}"))
}

fn load(
    store: &mut InstalledAppStore,
    expected: &VerifiedPackage,
) -> Result<VerifiedPackage, String> {
    let app_id = expected.manifest().app_id();
    let package = store
        .load(app_id)
        .map_err(|error| format!("load {}: {error:?}", app_id.as_str()))?
        .ok_or_else(|| format!("missing installed package {}", app_id.as_str()))?;
    if package.manifest().json() != expected.manifest().json()
        || !package.assets().eq(expected.assets())
    {
        return Err(format!("installed bytes changed for {}", app_id.as_str()));
    }
    Ok(package)
}

fn package(app_id: &str, message: &str, width: u32) -> Result<VerifiedPackage, String> {
    let html = format!(
        r#"<!doctype html><html lang="en"><meta charset="utf-8"><title>{app_id}</title><link rel="stylesheet" href="{STYLESHEET}"><h1 id="fixture-heading">{message}</h1></html>"#,
    );
    let css = format!("#fixture-heading {{ width: {width}px; }}");
    let held = BTreeMap::from([
        (ENTRYPOINT.to_owned(), html.into_bytes()),
        (STYLESHEET.to_owned(), css.into_bytes()),
    ]);
    let assets: Vec<_> = held
        .iter()
        .map(|(path, bytes)| {
            json!({
                "path": path,
                "sizeBytes": bytes.len(),
                "sha256": format!("{:x}", Sha256::digest(bytes)),
            })
        })
        .collect();
    let manifest = json!({
        "schemaVersion": 1,
        "appId": app_id,
        "packageVersion": "1.0.0",
        "entrypoint": ENTRYPOINT,
        "assets": assets,
        "requestedCapabilities": [],
    });
    let json = serde_json::to_vec(&manifest).map_err(|error| error.to_string())?;
    verify_package(json, held).map_err(|error| format!("verify fixture {app_id}: {error:?}"))
}
