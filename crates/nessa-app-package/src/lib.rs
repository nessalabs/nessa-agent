//! Verify static extension packages from held bytes without filesystem access.

mod domain;
mod infrastructure;

pub use domain::asset_integrity::{verify_asset, AssetIntegrityError, VerifiedAsset};
pub use domain::static_package::{
    PackageError, PackageManifest, RequestedCapability, VerifiedPackage, MAX_FILES,
    MAX_MANIFEST_BYTES, MAX_PATH_BYTES,
};
pub use infrastructure::inline::{
    encode_inline_package, scaffold_inline_package, validate_inline_package, verify_inline_package,
    InlineAsset, InlinePackage, InlineRefusal, ValidationReceipt,
};
pub use infrastructure::manifest::{parse_manifest, scaffold_package, verify_package};

#[cfg(feature = "disposable-store")]
pub use infrastructure::installed_store::{InstalledApp, InstalledAppStore, StoreError};
