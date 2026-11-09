//! One vault before any provider is configured. No native store is opened here.
use crate::product::VaultRoutes;
use nessa_vault::{
    application::{Clock, ObservationClock, Vault},
    infrastructure::{EncryptedFileVault, FileAudit, OsEntropy},
};
use std::{path::Path, sync::Arc, time::Instant};
struct Monotonic(Instant);
impl Clock for Monotonic {
    fn seconds(&self) -> u64 {
        self.0.elapsed().as_secs()
    }
}
pub(super) fn compose(namespace: &Path) -> Arc<VaultRoutes> {
    let vault = Vault::new(
        Arc::new(EncryptedFileVault::new(
            namespace.join("vault"),
            Arc::new(OsEntropy),
        )),
        Arc::new(FileAudit::new(
            namespace.join("vault/audit"),
            Arc::new(WallClock),
        )),
        Arc::new(Monotonic(Instant::now())),
    );
    Arc::new(VaultRoutes {
        vault,
        migration: Arc::new(crate::vault::infrastructure::LegacyGithubMigration::new(
            namespace.to_path_buf(),
            crate::secrets::KeychainSecret::new(namespace, "github-sign-in")
                .map(|entry| Arc::new(entry) as Arc<dyn crate::secrets::SecretStore>),
            crate::secrets::KeychainSecret::new(namespace, "github-inbox-key")
                .map(|entry| Arc::new(entry) as Arc<dyn crate::secrets::SecretStore>),
        )),
    })
}

struct WallClock;
impl ObservationClock for WallClock {
    fn unix_millis(&self) -> Result<u64, nessa_vault::domain::Failure> {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
            .ok_or(nessa_vault::domain::Failure::AuditUnavailable)
    }
}
