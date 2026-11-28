//! What each apps method does, in order, against the store and the audit
//! ports. No method here reads a path, a socket or the clock itself.
use std::sync::{Arc, Mutex};

use nessa_app_package::{
    encode_inline_package, scaffold_inline_package, verify_inline_package, InlinePackage,
    InlineRefusal, VerifiedPackage,
};
use nessa_types::app_identity::ExtensionAppId;
use uuid::Uuid;

/// Who asked: the owner's session, as the audit names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppsCaller {
    pub organization_id: String,
    pub principal_id: String,
    pub credential_id: String,
}

/// Why an apps method was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppsError {
    PackageInvalid,
    PackageTooLarge,
    ReviewStale,
    AlreadyInstalled,
    /// As many apps are installed as `apps.installed` can list.
    LimitReached,
    NotInstalled,
    StorageUnavailable,
    AuditUnavailable,
}

/// The most apps one gateway keeps: as many as `apps.installed` answers
/// (`maxItems` in the protocol). An install past it is refused, so the list
/// always fits.
pub const MAX_INSTALLED_APPS: usize = 256;

impl From<InlineRefusal> for AppsError {
    fn from(refusal: InlineRefusal) -> Self {
        match refusal {
            InlineRefusal::InlineLimit => Self::PackageTooLarge,
            InlineRefusal::InvalidEncoding
            | InlineRefusal::ManifestInvalid
            | InlineRefusal::AssetInventory
            | InlineRefusal::AssetLength
            | InlineRefusal::AssetDigest => Self::PackageInvalid,
        }
    }
}

/// The verifier's findings, as the owner reviews them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    pub app_id: String,
    pub package_version: String,
    pub entrypoint: String,
    pub asset_count: usize,
    pub digest: String,
}

impl Review {
    fn of(package: &VerifiedPackage) -> Self {
        let manifest = package.manifest();
        Self {
            app_id: manifest.app_id().as_str().to_owned(),
            package_version: manifest.package_version().to_owned(),
            entrypoint: manifest.entrypoint().to_owned(),
            asset_count: package.assets().count(),
            digest: package.digest(),
        }
    }
}

/// One installed app, as listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub app_id: String,
    pub package_version: String,
}

/// The store could not do what was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreRefusal {
    AlreadyInstalled,
    NotInstalled,
    Unavailable,
}

/// The private store of installed packages. Every load verifies the bytes.
pub trait AppStore: Send {
    fn install(&mut self, package: &VerifiedPackage) -> Result<(), StoreRefusal>;
    fn inventory(&mut self) -> Result<Vec<Installed>, StoreRefusal>;
    fn load(&mut self, app: &ExtensionAppId) -> Result<Option<VerifiedPackage>, StoreRefusal>;
    fn remove(&mut self, app: &ExtensionAppId) -> Result<(), StoreRefusal>;
}

/// A consequential change, at one of its phases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppsEvent {
    InstallRequested {
        digest: String,
        package_version: String,
    },
    Installed {
        digest: String,
    },
    InstallFailed {
        reason: &'static str,
    },
    UninstallRequested {
        package_version: String,
    },
    Uninstalled,
    UninstallFailed {
        reason: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppsRecord {
    pub operation_id: String,
    pub request_id: String,
    pub app_id: String,
    pub caller: AppsCaller,
    pub event: AppsEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditUnavailable;

/// Durable evidence of each install and uninstall.
pub trait AppsAudit: Send + Sync {
    fn record(&self, record: &AppsRecord) -> Result<(), AuditUnavailable>;
}

pub struct InstalledApps {
    /// One change or read at a time: SQLite is synchronous, and each change
    /// must see what the last one left.
    store: Arc<Mutex<Box<dyn AppStore>>>,
    audit: Arc<dyn AppsAudit>,
    /// One install or uninstall at a time, from its first read to its last
    /// record: what a change read is still there when it changes it.
    changes: tokio::sync::Mutex<()>,
}

/// A package and what the verifier found in it.
pub struct Checked {
    pub package: InlinePackage,
    pub review: Review,
}

impl InstalledApps {
    pub fn new(store: Box<dyn AppStore>, audit: Arc<dyn AppsAudit>) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            audit,
            changes: tokio::sync::Mutex::new(()),
        }
    }

    /// The starter package for a new app. Nothing is kept.
    pub fn starter(&self, app_id: String, package_version: String) -> Result<Checked, AppsError> {
        let package = scaffold_inline_package(app_id, package_version)?;
        let review = Review::of(&verify_inline_package(&package)?);
        Ok(Checked { package, review })
    }

    /// What the verifier finds in `package`. Nothing is kept.
    pub fn review(&self, package: &InlinePackage) -> Result<Review, AppsError> {
        Ok(Review::of(&verify_inline_package(package)?))
    }

    async fn with_store<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut dyn AppStore) -> T + Send + 'static,
    ) -> Result<T, AppsError> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            let mut store = store.lock().map_err(|_| AppsError::StorageUnavailable)?;
            Ok(work(store.as_mut()))
        })
        .await
        .map_err(|_| AppsError::StorageUnavailable)?
    }

    fn record(
        &self,
        operation_id: &str,
        request_id: &str,
        app_id: &str,
        caller: &AppsCaller,
        event: AppsEvent,
    ) -> Result<(), AppsError> {
        self.audit
            .record(&AppsRecord {
                operation_id: operation_id.to_owned(),
                request_id: request_id.to_owned(),
                app_id: app_id.to_owned(),
                caller: caller.clone(),
                event,
            })
            .map_err(|_| AppsError::AuditUnavailable)
    }

    /// A failure's record, once the change was refused. The refusal is what
    /// the caller hears; a record of it that cannot be kept is logged.
    fn record_failure(
        &self,
        operation_id: &str,
        request_id: &str,
        app_id: &str,
        caller: &AppsCaller,
        event: AppsEvent,
    ) {
        if self
            .record(operation_id, request_id, app_id, caller, event)
            .is_err()
        {
            tracing::error!(
                operation_id,
                app_id,
                "an apps failure could not be recorded"
            );
        }
    }

    /// Install `package`, only if its digest is the one the owner reviewed.
    /// Retrying after it installed answers the same app once the outcome is
    /// recorded: an install whose outcome could not be recorded is recorded
    /// by the retry, and a retry that cannot record it is refused too.
    pub async fn install(
        &self,
        caller: AppsCaller,
        request_id: String,
        package: &InlinePackage,
        reviewed_digest: &str,
    ) -> Result<Installed, AppsError> {
        let verified = verify_inline_package(package)?;
        let review = Review::of(&verified);
        if review.digest != reviewed_digest {
            return Err(AppsError::ReviewStale);
        }
        let app = verified.manifest().app_id().clone();
        let _change = self.changes.lock().await;
        let installed = Installed {
            app_id: review.app_id.clone(),
            package_version: review.package_version.clone(),
        };
        // The same bytes already installed: a retry, answered as it was.
        let held = {
            let app = app.clone();
            self.with_store(move |store| store.load(&app)).await?
        };
        match held {
            Ok(Some(existing)) if existing.digest() == review.digest => {
                // The bytes match, but only a record says the operation
                // ended: this retry's own outcome record says so.
                self.record(
                    &Uuid::new_v4().to_string(),
                    &request_id,
                    &review.app_id,
                    &caller,
                    AppsEvent::Installed {
                        digest: review.digest,
                    },
                )?;
                return Ok(installed);
            }
            Ok(Some(_)) => return Err(AppsError::AlreadyInstalled),
            Ok(None) => {}
            Err(_) => return Err(AppsError::StorageUnavailable),
        }
        // Counted under the change lock, so two installs cannot both pass.
        let count = self
            .with_store(|store| store.inventory())
            .await?
            .map_err(|_| AppsError::StorageUnavailable)?
            .len();
        if count >= MAX_INSTALLED_APPS {
            return Err(AppsError::LimitReached);
        }
        let operation_id = Uuid::new_v4().to_string();
        self.record(
            &operation_id,
            &request_id,
            &review.app_id,
            &caller,
            AppsEvent::InstallRequested {
                digest: review.digest.clone(),
                package_version: review.package_version.clone(),
            },
        )?;
        let outcome = self
            .with_store(move |store| store.install(&verified))
            .await
            .and_then(|outcome| {
                outcome.map_err(|refusal| match refusal {
                    StoreRefusal::AlreadyInstalled => AppsError::AlreadyInstalled,
                    StoreRefusal::NotInstalled | StoreRefusal::Unavailable => {
                        AppsError::StorageUnavailable
                    }
                })
            });
        if let Err(error) = outcome {
            self.record_failure(
                &operation_id,
                &request_id,
                &review.app_id,
                &caller,
                AppsEvent::InstallFailed {
                    reason: reason(error),
                },
            );
            return Err(error);
        }
        // Installed but not recorded is never answered as a success.
        self.record(
            &operation_id,
            &request_id,
            &review.app_id,
            &caller,
            AppsEvent::Installed {
                digest: review.digest,
            },
        )?;
        Ok(installed)
    }

    /// Every installed app, in identity order.
    pub async fn installed(&self) -> Result<Vec<Installed>, AppsError> {
        self.with_store(|store| store.inventory())
            .await?
            .map_err(|_| AppsError::StorageUnavailable)
    }

    /// One installed app's verified bytes.
    pub async fn read(&self, app: ExtensionAppId) -> Result<Checked, AppsError> {
        let verified = self
            .with_store(move |store| store.load(&app))
            .await?
            .map_err(|_| AppsError::StorageUnavailable)?
            .ok_or(AppsError::NotInstalled)?;
        let package = encode_inline_package(&verified)?;
        Ok(Checked {
            package,
            review: Review::of(&verified),
        })
    }

    /// Remove one app and everything it holds.
    pub async fn uninstall(
        &self,
        caller: AppsCaller,
        request_id: String,
        app: ExtensionAppId,
    ) -> Result<String, AppsError> {
        let app_id = app.as_str().to_owned();
        let _change = self.changes.lock().await;
        let held = {
            let app = app.clone();
            self.with_store(move |store| store.load(&app)).await?
        };
        let package_version = match held {
            Ok(Some(package)) => package.manifest().package_version().to_owned(),
            Ok(None) => return Err(AppsError::NotInstalled),
            Err(_) => return Err(AppsError::StorageUnavailable),
        };
        let operation_id = Uuid::new_v4().to_string();
        self.record(
            &operation_id,
            &request_id,
            &app_id,
            &caller,
            AppsEvent::UninstallRequested { package_version },
        )?;
        let outcome = self
            .with_store(move |store| store.remove(&app))
            .await
            .and_then(|outcome| {
                outcome.map_err(|refusal| match refusal {
                    StoreRefusal::NotInstalled => AppsError::NotInstalled,
                    StoreRefusal::AlreadyInstalled | StoreRefusal::Unavailable => {
                        AppsError::StorageUnavailable
                    }
                })
            });
        if let Err(error) = outcome {
            self.record_failure(
                &operation_id,
                &request_id,
                &app_id,
                &caller,
                AppsEvent::UninstallFailed {
                    reason: reason(error),
                },
            );
            return Err(error);
        }
        self.record(
            &operation_id,
            &request_id,
            &app_id,
            &caller,
            AppsEvent::Uninstalled,
        )?;
        Ok(app_id)
    }
}

fn reason(error: AppsError) -> &'static str {
    match error {
        AppsError::PackageInvalid => "package_invalid",
        AppsError::PackageTooLarge => "package_too_large",
        AppsError::ReviewStale => "review_stale",
        AppsError::AlreadyInstalled => "already_installed",
        AppsError::LimitReached => "limit_reached",
        AppsError::NotInstalled => "not_installed",
        AppsError::StorageUnavailable => "storage_unavailable",
        AppsError::AuditUnavailable => "audit_unavailable",
    }
}
