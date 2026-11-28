//! Verify static extension packages from held bytes without filesystem access.

mod domain;
mod infrastructure;

pub use domain::asset_integrity::{verify_asset, AssetIntegrityError, VerifiedAsset};
pub use domain::static_package::{
    AppServer, PackageError, PackageManifest, RequestedCapability, ServerRuntime, VerifiedPackage,
    MAX_FILES, MAX_MANIFEST_BYTES, MAX_PATH_BYTES,
};
pub use infrastructure::inline::{
    encode_inline_package, scaffold_inline_package, validate_inline_package, verify_inline_package,
    InlineAsset, InlinePackage, InlineRefusal, ValidationReceipt,
};
pub use infrastructure::launcher::{
    content_type, launcher_record, read_launcher_record, LauncherRefusal,
    APP_CONTENT_SECURITY_POLICY, LAUNCHER_RECORD, MAX_LAUNCHER_RECORD_BYTES, RUNNING_LOCK,
    RUNNING_PID,
};
pub use infrastructure::manifest::{parse_manifest, scaffold_package, verify_package};
pub use infrastructure::server::{
    find_runtime, ready_proof, start_server, AppCredentialRecord, RunningServer, ServerRefusal,
    APP_CREDENTIAL, MAX_APP_CREDENTIAL_BYTES, READY_PATH, SERVER_FOLDER, SERVER_LOG,
};

#[cfg(feature = "disposable-store")]
pub use infrastructure::installed_store::{InstalledApp, InstalledAppStore, StoreError};
