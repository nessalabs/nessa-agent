use std::collections::{BTreeMap, BTreeSet};

use nessa_types::app_identity::{ExtensionAppId, ExtensionAppIdError};

use super::asset_integrity::{verify_asset, AssetIntegrityError};

/// Maximum JSON manifest size before decoding.
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
/// Maximum number of declared static assets, including the entrypoint.
pub const MAX_FILES: usize = 128;
/// Maximum byte length of a relative asset path.
pub const MAX_PATH_BYTES: usize = 240;
const MAX_COMPONENT_BYTES: usize = 64;
const MAX_COMPONENTS: usize = 8;
const MAX_REQUESTED_CAPABILITIES: usize = 5;

/// Why a manifest and its held assets could not form a verified package.
#[derive(Debug, Eq, PartialEq)]
pub enum PackageError {
    ManifestTooLarge,
    MalformedManifest,
    UnsupportedVersion(u32),
    InvalidAppId(ExtensionAppIdError),
    InvalidPackageVersion,
    InvalidRequestedCapabilities,
    TooManyFiles,
    InvalidPath,
    ConflictingPaths,
    InvalidEntrypoint,
    InvalidIcon,
    InvalidDigest,
    MissingAsset,
    UndeclaredAsset,
    AssetIntegrity(AssetIntegrityError),
}

/// A requested capability declared by a package. This is not a permission.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RequestedCapability {
    List,
    Read,
    Create,
    Subscribe,
    Stop,
}

impl RequestedCapability {
    fn from_declaration(value: &str) -> Option<Self> {
        match value {
            "list" => Some(Self::List),
            "read" => Some(Self::Read),
            "create" => Some(Self::Create),
            "subscribe" => Some(Self::Subscribe),
            "stop" => Some(Self::Stop),
            _ => None,
        }
    }
}

pub(crate) struct AssetDeclaration {
    pub path: String,
    pub size_bytes: u64,
    pub sha256: [u8; 32],
}

/// Validated declarations, ready to be bound to their held asset bytes.
#[derive(Debug)]
pub struct PackageManifest {
    app_id: ExtensionAppId,
    package_version: String,
    entrypoint: String,
    icon: Option<String>,
    requested_capabilities: BTreeSet<RequestedCapability>,
    assets: BTreeMap<String, AssetIntegrity>,
    json: Vec<u8>,
}

#[derive(Debug)]
struct AssetIntegrity {
    size_bytes: u64,
    sha256: [u8; 32],
}

impl PackageManifest {
    /// The canonical extension app identity.
    pub fn app_id(&self) -> &ExtensionAppId {
        &self.app_id
    }

    /// The version label declared by the package.
    pub fn package_version(&self) -> &str {
        &self.package_version
    }

    /// The exact declared HTML entrypoint path.
    pub fn entrypoint(&self) -> &str {
        &self.entrypoint
    }

    /// The exact declared icon asset path, if present.
    pub fn icon(&self) -> Option<&str> {
        self.icon.as_deref()
    }

    /// The capabilities the package declares it would like to use.
    pub fn requested_capabilities(&self) -> &BTreeSet<RequestedCapability> {
        &self.requested_capabilities
    }

    /// The original validated JSON, preserved for later revalidation.
    pub fn json(&self) -> &[u8] {
        &self.json
    }

    /// Bind the declarations to exactly the supplied held bytes.
    ///
    /// # Errors
    ///
    /// Rejects missing, undeclared or changed assets.
    pub fn verify_held(
        self,
        assets: BTreeMap<String, Vec<u8>>,
    ) -> Result<VerifiedPackage, PackageError> {
        if assets.len() > MAX_FILES {
            return Err(PackageError::TooManyFiles);
        }
        if assets.keys().any(|path| !self.assets.contains_key(path)) {
            return Err(PackageError::UndeclaredAsset);
        }
        for (path, integrity) in &self.assets {
            let bytes = assets.get(path).ok_or(PackageError::MissingAsset)?;
            verify_asset(&self.app_id, bytes, integrity.size_bytes, integrity.sha256)
                .map_err(PackageError::AssetIntegrity)?;
        }
        Ok(VerifiedPackage {
            manifest: self,
            assets,
        })
    }
}

/// One owned record of a manifest and the exact static bytes it describes.
#[derive(Debug)]
pub struct VerifiedPackage {
    manifest: PackageManifest,
    assets: BTreeMap<String, Vec<u8>>,
}

impl VerifiedPackage {
    /// The validated manifest associated with the checked bytes.
    pub fn manifest(&self) -> &PackageManifest {
        &self.manifest
    }

    /// Look up verified bytes by their exact declared path.
    pub fn asset(&self, path: &str) -> Option<&[u8]> {
        self.assets.get(path).map(Vec::as_slice)
    }

    /// Iterate in path order over the verified assets.
    pub fn assets(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.assets
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
    }
}

pub(crate) struct ManifestDeclarations {
    pub app_id: ExtensionAppId,
    pub package_version: String,
    pub entrypoint: String,
    pub icon: Option<String>,
    pub requested_capabilities: Vec<String>,
    pub assets: Vec<AssetDeclaration>,
}

pub(crate) fn validate_declarations(
    input: ManifestDeclarations,
    json: Vec<u8>,
) -> Result<PackageManifest, PackageError> {
    if input.package_version.trim().is_empty() {
        return Err(PackageError::InvalidPackageVersion);
    }
    if input.assets.len() > MAX_FILES {
        return Err(PackageError::TooManyFiles);
    }
    if input.requested_capabilities.len() > MAX_REQUESTED_CAPABILITIES {
        return Err(PackageError::InvalidRequestedCapabilities);
    }
    let mut requested_capabilities = BTreeSet::new();
    for name in &input.requested_capabilities {
        let capability = RequestedCapability::from_declaration(name)
            .ok_or(PackageError::InvalidRequestedCapabilities)?;
        if !requested_capabilities.insert(capability) {
            return Err(PackageError::InvalidRequestedCapabilities);
        }
    }

    let mut assets = BTreeMap::new();
    for asset in input.assets {
        if !canonical(&asset.path) {
            return Err(PackageError::InvalidPath);
        }
        if assets
            .insert(
                asset.path,
                AssetIntegrity {
                    size_bytes: asset.size_bytes,
                    sha256: asset.sha256,
                },
            )
            .is_some()
        {
            return Err(PackageError::ConflictingPaths);
        }
    }
    for path in assets.keys() {
        if path
            .match_indices('/')
            .any(|(index, _)| assets.contains_key(&path[..index]))
        {
            return Err(PackageError::ConflictingPaths);
        }
    }
    if !input.entrypoint.ends_with(".html") || !assets.contains_key(&input.entrypoint) {
        return Err(PackageError::InvalidEntrypoint);
    }
    if input
        .icon
        .as_ref()
        .is_some_and(|path| !assets.contains_key(path))
    {
        return Err(PackageError::InvalidIcon);
    }

    Ok(PackageManifest {
        app_id: input.app_id,
        package_version: input.package_version,
        entrypoint: input.entrypoint,
        icon: input.icon,
        requested_capabilities,
        assets,
        json,
    })
}

fn canonical(path: &str) -> bool {
    path.len() <= MAX_PATH_BYTES
        && path.split('/').count() <= MAX_COMPONENTS
        && path.split('/').all(canonical_component)
}

fn canonical_component(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or_default();
    let reserved = matches!(stem, "con" | "prn" | "aux" | "nul")
        || ["com", "lpt"].iter().any(|prefix| {
            stem.strip_prefix(prefix)
                .is_some_and(|suffix| matches!(suffix.as_bytes(), [b'0'..=b'9']))
        });
    !reserved
        && part.len() <= MAX_COMPONENT_BYTES
        && part
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && !part.ends_with('.')
        && part.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
}
