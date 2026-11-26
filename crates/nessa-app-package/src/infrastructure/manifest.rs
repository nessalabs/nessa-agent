use std::{collections::BTreeMap, fmt, marker::PhantomData};

use nessa_types::app_identity::ExtensionAppId;
use serde::{
    de::{value::MapAccessDeserializer, MapAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};

use sha2::{Digest, Sha256};

use crate::domain::static_package::{self, AssetDeclaration, ManifestDeclarations};
use crate::{PackageError, PackageManifest, VerifiedPackage, MAX_MANIFEST_BYTES};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    app_id: String,
    package_version: String,
    entrypoint: String,
    icon: Option<String>,
    requested_capabilities: Vec<String>,
    assets: Vec<Object<Asset>>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Asset {
    path: String,
    size_bytes: u64,
    sha256: String,
}

// Serde's derived struct visitor also accepts positional sequences. Only the
// map representation is part of the provisional JSON contract.
#[derive(Serialize)]
#[serde(transparent)]
struct Object<T>(T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Object<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ObjectVisitor<T>(PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for ObjectVisitor<T> {
            type Value = Object<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<Self::Value, M::Error> {
                T::deserialize(MapAccessDeserializer::new(map)).map(Object)
            }
        }

        deserializer.deserialize_map(ObjectVisitor(PhantomData))
    }
}

const SCHEMA_VERSION: u32 = 1;
const STARTER_ENTRYPOINT: &str = "index.html";
const STARTER_HTML: &[u8] = b"<!doctype html>\n<html lang=\"en\">\n<meta charset=\"utf-8\">\n<title>Nessa app</title>\n<h1>Nessa app</h1>\n</html>\n";

/// Build one inert, deterministic static starter. The app ID and version are
/// written only to the manifest; they are not inserted into the HTML.
///
/// # Errors
/// Returns the same typed declaration/verification errors as `verify_package`.
pub fn scaffold_package(
    app_id: String,
    package_version: String,
) -> Result<VerifiedPackage, PackageError> {
    let manifest = Manifest {
        schema_version: SCHEMA_VERSION,
        app_id,
        package_version,
        entrypoint: STARTER_ENTRYPOINT.to_owned(),
        icon: None,
        requested_capabilities: Vec::new(),
        assets: vec![Object(Asset {
            path: STARTER_ENTRYPOINT.to_owned(),
            size_bytes: STARTER_HTML.len() as u64,
            sha256: hex::encode(Sha256::digest(STARTER_HTML)),
        })],
    };
    let json = serde_json::to_vec(&manifest).map_err(|_| PackageError::MalformedManifest)?;
    verify_package(
        json,
        BTreeMap::from([(STARTER_ENTRYPOINT.to_owned(), STARTER_HTML.to_vec())]),
    )
}

/// Validate one portable static package and take ownership of its bytes.
///
/// Inputs are the versioned JSON manifest and the held asset bytes indexed by
/// their manifest paths. The function reads no files and performs no mounting.
///
/// # Errors
///
/// Returns a typed error for unsupported or malformed manifests, invalid paths,
/// missing or extra assets, exceeded bounds, or a byte integrity mismatch.
pub fn verify_package(
    manifest: Vec<u8>,
    assets: BTreeMap<String, Vec<u8>>,
) -> Result<VerifiedPackage, PackageError> {
    parse_manifest(manifest)?.verify_held(assets)
}

/// Parse and validate provisional V1 declarations before reading any assets.
///
/// # Errors
///
/// Rejects unsupported versions, malformed declarations, and exceeded bounds.
pub fn parse_manifest(manifest: Vec<u8>) -> Result<PackageManifest, PackageError> {
    if manifest.len() > MAX_MANIFEST_BYTES {
        return Err(PackageError::ManifestTooLarge);
    }
    let Object(input): Object<Manifest> =
        serde_json::from_slice(&manifest).map_err(|_| PackageError::MalformedManifest)?;
    if input.schema_version != SCHEMA_VERSION {
        return Err(PackageError::UnsupportedVersion(input.schema_version));
    }
    let app_id = ExtensionAppId::new(input.app_id).map_err(PackageError::InvalidAppId)?;
    let declarations = input
        .assets
        .into_iter()
        .map(|Object(asset)| {
            let mut sha256 = [0; 32];
            if asset.sha256.bytes().any(|byte| byte.is_ascii_uppercase()) {
                return Err(PackageError::InvalidDigest);
            }
            hex::decode_to_slice(asset.sha256, &mut sha256)
                .map_err(|_| PackageError::InvalidDigest)?;
            Ok(AssetDeclaration {
                path: asset.path,
                size_bytes: asset.size_bytes,
                sha256,
            })
        })
        .collect::<Result<_, _>>()?;
    static_package::validate_declarations(
        ManifestDeclarations {
            app_id,
            package_version: input.package_version,
            entrypoint: input.entrypoint,
            icon: input.icon,
            requested_capabilities: input.requested_capabilities,
            assets: declarations,
        },
        manifest,
    )
}
