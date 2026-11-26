//! Verify static extension packages from held bytes without filesystem access.

mod domain;
mod infrastructure;

pub use domain::asset_integrity::{verify_asset, AssetIntegrityError, VerifiedAsset};
pub use domain::static_package::{
    PackageError, PackageManifest, RequestedCapability, VerifiedPackage, MAX_FILES,
    MAX_MANIFEST_BYTES, MAX_PATH_BYTES,
};
pub use infrastructure::manifest::{parse_manifest, verify_package};
