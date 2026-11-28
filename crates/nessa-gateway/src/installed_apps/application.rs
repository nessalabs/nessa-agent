//! What each apps method does, in order, against the store and the audit
//! ports. No method here reads a path, a socket or the clock itself.
use std::sync::{Arc, Mutex};

use nessa_app_package::{
    encode_inline_package, scaffold_inline_package, verify_inline_package, InlinePackage,
    InlineRefusal, RequestedCapability, VerifiedPackage,
};
use nessa_auth::application::app_credential::{AppCapabilities, AppCapability, AppCredentials};
use nessa_auth::application::ports::PortFuture;
use nessa_auth::application::session::AuthenticatedSession;
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
    /// The installed package requests no capability: it has nothing to
    /// sign in for.
    RequestsNothing,
    /// This request already issued a credential, whose secret is never
    /// answered twice.
    CredentialConflict,
    /// The credential store could not issue or revoke.
    CredentialsUnavailable,
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
    /// What the app asks to do in Nessa, in a fixed order. Installing it
    /// approves exactly these.
    pub requested_capabilities: Vec<AppCapability>,
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
            requested_capabilities: requested(package),
        }
    }
}

/// The capabilities a verified package requests, as the credential
/// store names them.
fn requested(package: &VerifiedPackage) -> Vec<AppCapability> {
    package
        .manifest()
        .requested_capabilities()
        .iter()
        .map(|capability| match capability {
            RequestedCapability::List => AppCapability::List,
            RequestedCapability::Read => AppCapability::Read,
            RequestedCapability::Create => AppCapability::Create,
            RequestedCapability::Subscribe => AppCapability::Subscribe,
            RequestedCapability::Stop => AppCapability::Stop,
            RequestedCapability::Send => AppCapability::Send,
        })
        .collect()
}

/// One credential issued to an app. `secret` is answered once and kept
/// nowhere in the gateway.
pub struct IssuedAppCredential {
    pub credential_id: String,
    pub secret: String,
    pub expires_at: u64,
    pub capabilities: Vec<AppCapability>,
}

/// Why the credential store refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialRefusal {
    /// The request already issued, and its secret cannot be answered again.
    Conflict,
    Unavailable,
}

/// The credential store as installed apps use it: the owner issues and
/// revokes an app's credentials, and an app's session is checked against
/// the same store.
pub trait AppCredentialAuthority: Send + Sync {
    /// Revoke every live credential of `app`, then issue one holding exactly
    /// `capabilities`, approved by `owner`.
    fn issue<'a>(
        &'a self,
        owner: &'a AuthenticatedSession,
        request_id: String,
        app: ExtensionAppId,
        capabilities: AppCapabilities,
    ) -> PortFuture<'a, IssuedAppCredential, CredentialRefusal>;

    /// Revoke every live credential of `app` in `organization` for
    /// `issuer`, the owner's principal (admitted to `credential.manage`
    /// before this is asked); how many there were.
    fn revoke_all<'a>(
        &'a self,
        organization: String,
        issuer: String,
        request_id: String,
        app: ExtensionAppId,
    ) -> PortFuture<'a, usize, CredentialRefusal>;

    /// App proof and revalidation, for an app's own session.
    fn sessions(&self) -> &dyn AppCredentials;
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
    /// A credential was issued to the app, holding these capabilities.
    CredentialIssued {
        credential_id: String,
        capabilities: Vec<&'static str>,
    },
    /// The app's live credentials were revoked.
    CredentialsRevoked {
        revoked: usize,
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
    /// The credential store, when this gateway issues app credentials.
    credentials: Option<Arc<dyn AppCredentialAuthority>>,
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
            credentials: None,
            changes: tokio::sync::Mutex::new(()),
        }
    }

    /// Issue and revoke app credentials through `credentials`. Without it,
    /// an app has nothing to sign in with.
    pub fn with_credentials(mut self, credentials: Arc<dyn AppCredentialAuthority>) -> Self {
        self.credentials = Some(credentials);
        self
    }

    /// The credential store an app's session is checked against.
    pub fn credentials(&self) -> Option<&Arc<dyn AppCredentialAuthority>> {
        self.credentials.as_ref()
    }

    /// Issue `app` a credential holding exactly what its installed package
    /// requests, revoking any it held before. Under the change lock, so an
    /// uninstall cannot pass between the read and the issue.
    pub async fn issue_credential(
        &self,
        owner: &AuthenticatedSession,
        caller: AppsCaller,
        request_id: String,
        app: ExtensionAppId,
    ) -> Result<IssuedAppCredential, AppsError> {
        let credentials = self
            .credentials
            .as_ref()
            .ok_or(AppsError::CredentialsUnavailable)?;
        let _change = self.changes.lock().await;
        let held = {
            let app = app.clone();
            self.with_store(move |store| store.load(&app)).await?
        };
        let package = match held {
            Ok(Some(package)) => package,
            Ok(None) => return Err(AppsError::NotInstalled),
            Err(_) => return Err(AppsError::StorageUnavailable),
        };
        let requested = requested(&package);
        let [first, rest @ ..] = requested.as_slice() else {
            return Err(AppsError::RequestsNothing);
        };
        let capabilities = AppCapabilities::new(*first, rest);
        let issued = credentials
            .issue(owner, request_id.clone(), app.clone(), capabilities)
            .await
            .map_err(credential_error)?;
        // A credential with no issued record is not handed out: take it back
        // (the app keeps nothing live) and say the audit is unavailable.
        let recorded = self.record(
            &Uuid::new_v4().to_string(),
            &request_id,
            app.as_str(),
            &caller,
            AppsEvent::CredentialIssued {
                credential_id: issued.credential_id.clone(),
                capabilities: issued
                    .capabilities
                    .iter()
                    .map(|capability| capability_name(*capability))
                    .collect(),
            },
        );
        if let Err(error) = recorded {
            if let Err(refusal) = credentials
                .revoke_all(
                    caller.organization_id.clone(),
                    caller.principal_id.clone(),
                    format!("{request_id}:unrecorded"),
                    app.clone(),
                )
                .await
            {
                tracing::error!(
                    app = app.as_str(),
                    ?refusal,
                    "an app credential with no issued record could not be revoked"
                );
            }
            return Err(error);
        }
        Ok(issued)
    }

    /// Revoke every credential `app` holds; how many were live.
    pub async fn revoke_credentials(
        &self,
        caller: AppsCaller,
        request_id: String,
        app: ExtensionAppId,
    ) -> Result<usize, AppsError> {
        let _change = self.changes.lock().await;
        self.revoke_held(&caller, request_id, app).await
    }

    async fn revoke_held(
        &self,
        caller: &AppsCaller,
        request_id: String,
        app: ExtensionAppId,
    ) -> Result<usize, AppsError> {
        let Some(credentials) = self.credentials.as_ref() else {
            return Ok(0);
        };
        let revoked = credentials
            .revoke_all(
                caller.organization_id.clone(),
                caller.principal_id.clone(),
                request_id.clone(),
                app.clone(),
            )
            .await
            .map_err(credential_error)?;
        // The revocation stands either way; a lost record is reported, and a
        // retry under the same request id replays the count and records it.
        if revoked > 0 {
            self.record(
                &Uuid::new_v4().to_string(),
                &request_id,
                app.as_str(),
                caller,
                AppsEvent::CredentialsRevoked { revoked },
            )?;
        }
        Ok(revoked)
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

    /// A record of a refused change. What the caller hears does not change;
    /// a record that cannot be kept is logged.
    fn record_or_log(
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
            tracing::error!(operation_id, app_id, "an apps record could not be kept");
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
            self.record_or_log(
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

    /// Remove one app and everything it holds. Its credentials are revoked
    /// first: an app that could still sign in is never answered as removed.
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
        self.revoke_held(&caller, request_id.clone(), app.clone())
            .await?;
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
            self.record_or_log(
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
        AppsError::RequestsNothing => "requests_nothing",
        AppsError::CredentialConflict => "credential_conflict",
        AppsError::CredentialsUnavailable => "credentials_unavailable",
    }
}

fn credential_error(refusal: CredentialRefusal) -> AppsError {
    match refusal {
        CredentialRefusal::Conflict => AppsError::CredentialConflict,
        CredentialRefusal::Unavailable => AppsError::CredentialsUnavailable,
    }
}

/// A capability's wire name.
pub fn capability_name(capability: AppCapability) -> &'static str {
    match capability {
        AppCapability::List => "list",
        AppCapability::Read => "read",
        AppCapability::Create => "create",
        AppCapability::Subscribe => "subscribe",
        AppCapability::Stop => "stop",
        AppCapability::Send => "send",
    }
}
