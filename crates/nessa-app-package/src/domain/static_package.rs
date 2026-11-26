use std::collections::{BTreeMap, BTreeSet};

use nessa_types::app_identity::{ExtensionAppId, ExtensionAppIdError};

use super::asset_integrity::{verify_asset, AssetIntegrityError};

/// Maximum total bytes across the portable package's assets.
pub const MAX_ASSET_BYTES: usize = 64 * 1024;
/// Maximum number of declared static assets, including the entrypoint.
pub const MAX_FILES: usize = 128;
/// Maximum JSON manifest size before decoding.
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;

/// Why a manifest and its held assets could not form a verified package.
#[derive(Debug, Eq, PartialEq)]
pub enum PackageError {
    ManifestTooLarge,
    MalformedManifest,
    UnsupportedVersion(u32),
    InvalidAppId(ExtensionAppIdError),
    TooManyFiles,
    TooManyAssetBytes,
    InvalidPath,
    ConflictingPaths,
    InvalidEntrypoint,
    InvalidDigest,
    MissingAsset,
    UndeclaredAsset,
    AssetIntegrity(AssetIntegrityError),
}

/// One owned record of a manifest and the exact static bytes it describes.
///
/// The verified record conveys integrity, without granting authority or
/// establishing the publisher's identity.
#[derive(Debug)]
pub struct VerifiedPackage {
    app_id: ExtensionAppId,
    entrypoint: String,
    manifest: Vec<u8>,
    assets: BTreeMap<String, Vec<u8>>,
}

impl VerifiedPackage {
    /// The canonical extension app identity declared by the manifest.
    pub fn app_id(&self) -> &ExtensionAppId {
        &self.app_id
    }

    /// The exact declared HTML entrypoint path.
    pub fn entrypoint(&self) -> &str {
        &self.entrypoint
    }

    /// The validated JSON bytes, preserved for persistence and revalidation.
    pub fn manifest(&self) -> &[u8] {
        &self.manifest
    }

    /// Look up verified bytes by their exact declared path.
    pub fn asset(&self, path: &str) -> Option<&[u8]> {
        self.assets.get(path).map(Vec::as_slice)
    }

    /// Iterate in path order over assets and their verified bytes.
    pub fn assets(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.assets
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
    }
}

pub(crate) struct AssetDeclaration {
    pub path: String,
    pub byte_count: u64,
    pub sha256: [u8; 32],
}

pub(crate) fn verify(
    app_id: ExtensionAppId,
    entrypoint: String,
    declarations: Vec<AssetDeclaration>,
    manifest: Vec<u8>,
    assets: BTreeMap<String, Vec<u8>>,
) -> Result<VerifiedPackage, PackageError> {
    if declarations.len() > MAX_FILES || assets.len() > MAX_FILES {
        return Err(PackageError::TooManyFiles);
    }

    let actual_size = assets
        .values()
        .try_fold(0usize, |total, bytes| total.checked_add(bytes.len()));
    let declared_size = declarations
        .iter()
        .try_fold(0u64, |total, asset| total.checked_add(asset.byte_count));
    if actual_size.is_none_or(|size| size > MAX_ASSET_BYTES)
        || declared_size.is_none_or(|size| size > MAX_ASSET_BYTES as u64)
    {
        return Err(PackageError::TooManyAssetBytes);
    }

    let mut paths = BTreeSet::new();
    for asset in &declarations {
        if !canonical(&asset.path) {
            return Err(PackageError::InvalidPath);
        }
        if !paths.insert(asset.path.as_str()) {
            return Err(PackageError::ConflictingPaths);
        }
    }
    for path in &paths {
        if path
            .match_indices('/')
            .any(|(index, _)| paths.contains(&path[..index]))
        {
            return Err(PackageError::ConflictingPaths);
        }
    }
    if !entrypoint.ends_with(".html") || !paths.contains(entrypoint.as_str()) {
        return Err(PackageError::InvalidEntrypoint);
    }
    if assets.keys().any(|path| !paths.contains(path.as_str())) {
        return Err(PackageError::UndeclaredAsset);
    }
    for asset in declarations {
        let bytes = assets.get(&asset.path).ok_or(PackageError::MissingAsset)?;
        verify_asset(&app_id, bytes, asset.byte_count, asset.sha256)
            .map_err(PackageError::AssetIntegrity)?;
    }

    Ok(VerifiedPackage {
        app_id,
        entrypoint,
        manifest,
        assets,
    })
}

fn canonical(path: &str) -> bool {
    path.split('/').all(|part| {
        let stem = part.split('.').next().unwrap_or_default();
        let reserved = matches!(stem, "con" | "prn" | "aux" | "nul")
            || ["com", "lpt"].iter().any(|prefix| {
                stem.strip_prefix(prefix)
                    .is_some_and(|suffix| matches!(suffix.as_bytes(), [b'0'..=b'9']))
            });
        !reserved
            && part
                .as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            && !part.ends_with('.')
            && part.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
            })
    })
}
