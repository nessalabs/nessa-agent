use std::collections::BTreeMap;

use nessa_app_package::{
    parse_manifest, verify_package, AssetIntegrityError, PackageError, RequestedCapability,
    ServerRuntime, MAX_FILES, MAX_MANIFEST_BYTES,
};
use nessa_types::app_identity::ExtensionAppIdError;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn declaration(path: &str, bytes: &[u8]) -> Value {
    json!({"path": path, "sizeBytes": bytes.len(), "sha256": hex::encode(Sha256::digest(bytes))})
}

fn manifest(assets: Vec<Value>) -> Value {
    json!({"schemaVersion": 1, "appId": "calendar", "packageVersion": "1.0.0", "entrypoint": "index.html", "requestedCapabilities": [], "assets": assets})
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
    assert_eq!(record.manifest().app_id().as_str(), "calendar");
    assert_eq!(record.manifest().entrypoint(), "index.html");
    assert_eq!(record.manifest().json(), manifest);
    assert_eq!(
        record.asset(record.manifest().entrypoint()),
        Some(b"abc".as_slice())
    );
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
    let duplicate = br#"{"schemaVersion":1,"schemaVersion":1,"appId":"calendar","packageVersion":"1","entrypoint":"index.html","requestedCapabilities":[],"assets":[]}"#;
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
fn applies_the_byte_ceiling_to_the_manifest_and_not_to_held_assets() {
    let mut assets =
        BTreeMap::from([("index.html".to_string(), vec![b'x'; MAX_MANIFEST_BYTES + 1])]);
    assets.extend((1..MAX_FILES).map(|i| (format!("asset-{i}.css"), vec![])));
    let input = encoded(&manifest(
        assets
            .iter()
            .map(|(path, bytes)| declaration(path, bytes))
            .collect(),
    ));
    assert!(verify_package(input.clone(), assets.clone()).is_ok());

    assets.insert("extra.css".into(), vec![]);
    assert_eq!(
        verify_package(input, assets).unwrap_err(),
        PackageError::TooManyFiles
    );
    assert_eq!(
        parse_manifest(vec![b' '; MAX_MANIFEST_BYTES + 1]).unwrap_err(),
        PackageError::ManifestTooLarge
    );
}

#[test]
fn parses_version_icon_and_unique_capability_declarations_before_assets() {
    let mut input = manifest(vec![
        declaration("index.html", b"abc"),
        declaration("icon.svg", b"icon"),
    ]);
    input["icon"] = json!("icon.svg");
    input["requestedCapabilities"] = json!(["list", "read", "create", "subscribe", "stop"]);
    let parsed = parse_manifest(encoded(&input)).unwrap();
    assert_eq!(parsed.package_version(), "1.0.0");
    assert_eq!(parsed.icon(), Some("icon.svg"));
    assert_eq!(parsed.requested_capabilities().len(), 5);
    assert!(parsed
        .requested_capabilities()
        .contains(&RequestedCapability::Subscribe));
    assert_eq!(
        parsed.verify_held(held()).unwrap_err(),
        PackageError::MissingAsset
    );

    for invalid in [
        json!(["stop", "stop"]),
        json!(["admin"]),
        json!(["list", "read", "create", "subscribe", "stop", "list"]),
    ] {
        input["requestedCapabilities"] = invalid;
        assert_eq!(
            parse_manifest(encoded(&input)).unwrap_err(),
            PackageError::InvalidRequestedCapabilities
        );
    }
    input["requestedCapabilities"] = json!([]);
    input["icon"] = json!("other.svg");
    assert_eq!(
        parse_manifest(encoded(&input)).unwrap_err(),
        PackageError::InvalidIcon
    );
}

#[test]
fn bounds_path_bytes_components_and_depth() {
    let longest = format!(
        "{}/{}/{}/{}",
        "a".repeat(64),
        "b".repeat(64),
        "c".repeat(64),
        "d".repeat(45)
    );
    let deepest = "a/b/c/d/e/f/g/h";
    for path in [&longest, deepest] {
        let input = manifest(vec![
            declaration("index.html", b"abc"),
            declaration(path, b""),
        ]);
        assert!(parse_manifest(encoded(&input)).is_ok());
    }
    for path in [
        format!("{longest}d"),
        "a".repeat(65),
        format!("{deepest}/i"),
    ] {
        let input = manifest(vec![
            declaration("index.html", b"abc"),
            declaration(&path, b""),
        ]);
        assert_eq!(
            parse_manifest(encoded(&input)).unwrap_err(),
            PackageError::InvalidPath
        );
    }
}

#[test]
fn refuses_positional_manifest_and_asset_arrays() {
    let asset = declaration("index.html", b"abc");
    let positional_manifest = json!([1, "calendar", "1", "index.html", null, [], [asset]]);
    let positional_asset = json!(["index.html", 3, asset["sha256"]]);
    for input in [positional_manifest, manifest(vec![positional_asset])] {
        assert_eq!(
            parse_manifest(encoded(&input)).unwrap_err(),
            PackageError::MalformedManifest
        );
    }
}

#[test]
fn refuses_more_than_128_manifest_declarations_before_held_verification() {
    let mut assets = vec![declaration("index.html", b"")];
    assets.extend((1..128).map(|index| declaration(&format!("{index}.css"), b"")));
    assert!(parse_manifest(encoded(&manifest(assets.clone()))).is_ok());
    assets.push(declaration("extra.css", b""));
    assert_eq!(
        parse_manifest(encoded(&manifest(assets))).unwrap_err(),
        PackageError::TooManyFiles
    );
}

fn with_server(entry: &str, runtime: &str) -> (Vec<u8>, BTreeMap<String, Vec<u8>>) {
    let mut assets = held();
    assets.insert("server/main.mjs".into(), b"export {}".to_vec());
    assets.insert("server/main.ts".into(), b"export {}".to_vec());
    assets.insert("server/notes.txt".into(), b"hi".to_vec());
    let mut input = manifest(
        assets
            .iter()
            .map(|(path, bytes)| declaration(path, bytes))
            .collect(),
    );
    input["server"] = json!({"entry": entry, "runtime": runtime});
    input["requestedCapabilities"] = json!(["list"]);
    (encoded(&input), assets)
}

#[test]
fn reads_a_declared_server_entry_run_by_node_or_bun() {
    let (manifest, assets) = with_server("server/main.mjs", "node");
    let record = verify_package(manifest, assets).unwrap();
    let server = record.manifest().server().unwrap();
    assert_eq!(server.entry(), "server/main.mjs");
    assert_eq!(server.runtime(), ServerRuntime::Node);
    let (manifest, assets) = with_server("server/main.ts", "bun");
    let record = verify_package(manifest, assets).unwrap();
    assert_eq!(
        record.manifest().server().unwrap().runtime(),
        ServerRuntime::Bun
    );
    // A package without one is a static app.
    let plain = encoded(&manifest_of_held());
    assert!(verify_package(plain, held())
        .unwrap()
        .manifest()
        .server()
        .is_none());
}

fn manifest_of_held() -> Value {
    manifest(vec![declaration("index.html", b"abc")])
}

#[test]
fn refuses_a_server_that_is_not_a_declared_script_for_its_runtime() {
    for (entry, runtime) in [
        ("server/main.mjs", "deno"),
        ("server/main.mjs", "Node"),
        ("server/main.ts", "node"),
        ("server/notes.txt", "node"),
        ("server/missing.mjs", "node"),
        ("index.html", "node"),
    ] {
        let (manifest, assets) = with_server(entry, runtime);
        assert_eq!(
            verify_package(manifest, assets).unwrap_err(),
            PackageError::InvalidServer,
            "{entry} {runtime}"
        );
    }
    // A server signs in as the app, so a package asking for nothing in
    // Nessa cannot have one.
    let (manifest, assets) = with_server("server/main.mjs", "node");
    let mut input: Value = serde_json::from_slice(&manifest).unwrap();
    input["requestedCapabilities"] = json!([]);
    assert_eq!(
        verify_package(encoded(&input), assets).unwrap_err(),
        PackageError::InvalidServer
    );
    // The declaration takes exactly its two fields.
    let (manifest, assets) = with_server("server/main.mjs", "node");
    let mut input: Value = serde_json::from_slice(&manifest).unwrap();
    input["server"]["args"] = json!(["--inspect"]);
    assert_eq!(
        verify_package(encoded(&input), assets).unwrap_err(),
        PackageError::MalformedManifest
    );
}

#[test]
fn a_dynamic_starter_holds_its_server_and_asks_for_what_it_uses() {
    let starter = nessa_app_package::scaffold_package(
        "notes".into(),
        "1.0.0".into(),
        Some(ServerRuntime::Bun),
    )
    .unwrap();
    let server = starter.manifest().server().unwrap();
    assert_eq!(server.runtime(), ServerRuntime::Bun);
    let main = std::str::from_utf8(starter.asset(server.entry()).unwrap()).unwrap();
    assert!(main.contains("from \"@nessa/client\""));
    assert_eq!(
        starter
            .manifest()
            .requested_capabilities()
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        [
            RequestedCapability::List,
            RequestedCapability::Read,
            RequestedCapability::Create,
            RequestedCapability::Send,
        ]
    );
    // It fits the inline profile Settings and the CLI exchange.
    nessa_app_package::scaffold_inline_package(
        "notes".into(),
        "1.0.0".into(),
        Some(ServerRuntime::Node),
    )
    .unwrap();
    let plain = nessa_app_package::scaffold_package("notes".into(), "1.0.0".into(), None).unwrap();
    assert!(plain.manifest().server().is_none());
    assert!(plain.manifest().requested_capabilities().is_empty());
}
