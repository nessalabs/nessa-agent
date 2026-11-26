use std::collections::BTreeMap;

use nessa_app_package::{
    verify_package, AssetIntegrityError, PackageError, MAX_ASSET_BYTES, MAX_FILES,
    MAX_MANIFEST_BYTES,
};
use nessa_types::app_identity::ExtensionAppIdError;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn declaration(path: &str, bytes: &[u8]) -> Value {
    json!({"path": path, "byteCount": bytes.len(), "sha256": hex::encode(Sha256::digest(bytes))})
}

fn manifest(assets: Vec<Value>) -> Value {
    json!({"schemaVersion": 1, "appId": "calendar", "entrypoint": "index.html", "assets": assets})
}

fn held() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([("index.html".into(), b"abc".to_vec())])
}

fn encoded(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

#[test]
fn returns_one_owned_record_with_the_exact_declared_assets() {
    let mut assets = held();
    assets.insert("assets/app.css".into(), b"body{}".to_vec());
    let manifest = encoded(&manifest(
        assets
            .iter()
            .map(|(path, bytes)| declaration(path, bytes))
            .collect(),
    ));
    let record = verify_package(manifest.clone(), assets).unwrap();
    assert_eq!(record.app_id().as_str(), "calendar");
    assert_eq!(record.entrypoint(), "index.html");
    assert_eq!(record.manifest(), manifest);
    assert_eq!(record.asset(record.entrypoint()), Some(b"abc".as_slice()));
    assert_eq!(record.asset("assets/app.css"), Some(b"body{}".as_slice()));
    assert_eq!(record.assets().count(), 2);
}

#[test]
fn refuses_unknown_fields_at_both_levels_and_duplicate_fields() {
    let baseline = manifest(vec![declaration("index.html", b"abc")]);
    for field in ["permissions", "credentials", "command", "grants"] {
        let mut input = baseline.clone();
        input[field] = json!([]);
        assert_eq!(
            verify_package(encoded(&input), held()).unwrap_err(),
            PackageError::MalformedManifest
        );
    }
    let mut input = baseline;
    input["assets"][0]["live"] = json!(true);
    assert_eq!(
        verify_package(encoded(&input), held()).unwrap_err(),
        PackageError::MalformedManifest
    );
    let duplicate = br#"{"schemaVersion":1,"schemaVersion":1,"appId":"calendar","entrypoint":"index.html","assets":[]}"#;
    assert_eq!(
        verify_package(duplicate.to_vec(), held()).unwrap_err(),
        PackageError::MalformedManifest
    );
}

#[test]
fn refuses_an_unsupported_version_and_the_reserved_app_id() {
    let mut input = manifest(vec![declaration("index.html", b"abc")]);
    input["schemaVersion"] = json!(2);
    assert_eq!(
        verify_package(encoded(&input), held()).unwrap_err(),
        PackageError::UnsupportedVersion(2)
    );
    input["schemaVersion"] = json!(1);
    input["appId"] = json!("nessa");
    assert_eq!(
        verify_package(encoded(&input), held()).unwrap_err(),
        PackageError::InvalidAppId(ExtensionAppIdError::Reserved)
    );
}

#[test]
fn rejects_path_aliases_and_file_directory_conflicts() {
    for path in [
        "/x.js", "../x.js", "a/./x.js", "a//x.js", "x\\y.js", "x%2fy.js", "x.js?y", "A.js",
        "con.html", "x.",
    ] {
        let input = manifest(vec![
            declaration("index.html", b"abc"),
            declaration(path, b""),
        ]);
        assert_eq!(
            verify_package(encoded(&input), held()).unwrap_err(),
            PackageError::InvalidPath,
            "{path}"
        );
    }
    for path in ["index.html", "index.html/nested.js"] {
        let input = manifest(vec![
            declaration("index.html", b"abc"),
            declaration(path, b""),
        ]);
        assert_eq!(
            verify_package(encoded(&input), held()).unwrap_err(),
            PackageError::ConflictingPaths
        );
    }
}

#[test]
fn requires_a_declared_html_entrypoint_without_path_normalization() {
    let mut input = manifest(vec![
        declaration("index.html", b"abc"),
        declaration("app.js", b""),
    ]);
    for entry in ["missing.html", "app.js", "./index.html"] {
        input["entrypoint"] = json!(entry);
        assert_eq!(
            verify_package(encoded(&input), held()).unwrap_err(),
            PackageError::InvalidEntrypoint
        );
    }
}

#[test]
fn rejects_missing_extra_short_and_tampered_assets() {
    let input = encoded(&manifest(vec![declaration("index.html", b"abc")]));
    for (assets, expected) in [
        (BTreeMap::new(), PackageError::MissingAsset),
        (
            BTreeMap::from([
                ("index.html".into(), b"abc".to_vec()),
                ("extra.js".into(), vec![]),
            ]),
            PackageError::UndeclaredAsset,
        ),
        (
            BTreeMap::from([("index.html".into(), b"ab".to_vec())]),
            PackageError::AssetIntegrity(AssetIntegrityError::LengthMismatch),
        ),
        (
            BTreeMap::from([("index.html".into(), b"abd".to_vec())]),
            PackageError::AssetIntegrity(AssetIntegrityError::DigestMismatch),
        ),
    ] {
        assert_eq!(verify_package(input.clone(), assets).unwrap_err(), expected);
    }
}

#[test]
fn rejects_bad_digest_encoding() {
    let mut input = manifest(vec![declaration("index.html", b"abc")]);
    for digest in ["ab".to_string(), "g".repeat(64), "A".repeat(64)] {
        input["assets"][0]["sha256"] = json!(digest);
        assert_eq!(
            verify_package(encoded(&input), held()).unwrap_err(),
            PackageError::InvalidDigest
        );
    }
}

#[test]
fn accepts_the_portable_ceiling_and_rejects_each_overflow() {
    let mut assets = BTreeMap::from([("index.html".to_string(), vec![b'x'; MAX_ASSET_BYTES])]);
    assets.extend((1..MAX_FILES).map(|i| (format!("asset-{i}.css"), vec![])));
    let input = encoded(&manifest(
        assets
            .iter()
            .map(|(path, bytes)| declaration(path, bytes))
            .collect(),
    ));
    assert!(verify_package(input.clone(), assets.clone()).is_ok());

    assets.get_mut("index.html").unwrap().push(b'x');
    assert_eq!(
        verify_package(input.clone(), assets.clone()).unwrap_err(),
        PackageError::TooManyAssetBytes
    );
    assets.insert("extra.css".into(), vec![]);
    assert_eq!(
        verify_package(input, assets).unwrap_err(),
        PackageError::TooManyFiles
    );
    assert_eq!(
        verify_package(vec![b' '; MAX_MANIFEST_BYTES + 1], held()).unwrap_err(),
        PackageError::ManifestTooLarge
    );
}
