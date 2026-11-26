use std::collections::BTreeMap;

use nessa_types::app_identity::ExtensionAppId;
use serde::Deserialize;

use crate::domain::static_package::{self, AssetDeclaration, ManifestDeclarations};
use crate::{PackageError, PackageManifest, VerifiedPackage, MAX_MANIFEST_BYTES};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    app_id: String,
    package_version: String,
    entrypoint: String,
    icon: Option<String>,
    requested_capabilities: Vec<String>,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Asset {
    path: String,
    size_bytes: u64,
    sha256: String,
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
    let input: Manifest =
        serde_json::from_slice(&manifest).map_err(|_| PackageError::MalformedManifest)?;
    if input.schema_version != 1 {
        return Err(PackageError::UnsupportedVersion(input.schema_version));
    }
    let app_id = ExtensionAppId::new(input.app_id).map_err(PackageError::InvalidAppId)?;
    let declarations = input
        .assets
        .into_iter()
        .map(|asset| {
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
