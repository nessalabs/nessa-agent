//! [`AppStore`] over `nessa-app-package`'s private SQLite store. The store
//! verifies every package it loads; this only maps its refusals.
use std::path::Path;

use nessa_app_package::{InstalledAppStore, StoreError, VerifiedPackage};
use nessa_types::app_identity::ExtensionAppId;

use crate::installed_apps::application::{AppStore, Installed, StoreRefusal};

pub struct SqliteAppStore {
    store: InstalledAppStore,
}

impl SqliteAppStore {
    /// Open, or create, the store at `path`.
    ///
    /// # Errors
    /// The store's own open failure, for composition to log.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        Ok(Self {
            store: InstalledAppStore::open(path)?,
        })
    }
}

fn refusal(error: StoreError) -> StoreRefusal {
    match error {
        StoreError::AlreadyInstalled => StoreRefusal::AlreadyInstalled,
        StoreError::NotInstalled => StoreRefusal::NotInstalled,
        other => {
            tracing::error!(error = ?other, "the installed-app store refused");
            StoreRefusal::Unavailable
        }
    }
}

impl AppStore for SqliteAppStore {
    fn install(&mut self, package: &VerifiedPackage) -> Result<(), StoreRefusal> {
        self.store.install(package).map_err(refusal)
    }

    fn inventory(&mut self) -> Result<Vec<Installed>, StoreRefusal> {
        Ok(self
            .store
            .inventory()
            .map_err(refusal)?
            .into_iter()
            .map(|app| Installed {
                app_id: app.app_id().as_str().to_owned(),
                package_version: app.package_version().to_owned(),
            })
            .collect())
    }

    fn load(&mut self, app: &ExtensionAppId) -> Result<Option<VerifiedPackage>, StoreRefusal> {
        self.store.load(app).map_err(refusal)
    }

    fn remove(&mut self, app: &ExtensionAppId) -> Result<(), StoreRefusal> {
        self.store.remove(app).map_err(refusal)
    }
}
