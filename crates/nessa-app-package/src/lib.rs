//! Verify static extension packages from held bytes without filesystem access.

mod domain;
mod infrastructure;

pub use domain::asset_integrity::{verify_asset, AssetIntegrityError, VerifiedAsset};
pub use domain::static_package::{
    PackageError, VerifiedPackage, MAX_ASSET_BYTES, MAX_FILES, MAX_MANIFEST_BYTES,
};
pub use infrastructure::manifest::verify_package;
