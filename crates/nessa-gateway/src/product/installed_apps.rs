//! The installed-app methods (Cedar has admitted `credential.manage`, so the
//! caller is the owner), translated into [`InstalledApps`]:
//!
//! ```text
//! apps.starter {appId, packageVersion} ──▶ { package, review }   nothing kept
//! apps.review {package}                ──▶ AppReview             nothing kept
//! apps.install {requestId, package, reviewedDigest}
//!     ──▶ verified again ──▶ digest == reviewedDigest ──▶ audited install ──▶ { app }
//! apps.installed                       ──▶ { apps }  (empty means none)
//! apps.read {appId}                    ──▶ { package, review }  bytes verified on load
//! apps.uninstall {requestId, appId}    ──▶ revoke its credentials ──▶ audited remove ──▶ { appId }
//! apps.issueCredential {requestId, appId}
//!     ──▶ installed? ──▶ revoke the old ──▶ issue exactly its requested capabilities
//! apps.revokeCredentials {requestId, appId} ──▶ { revoked }
//! ```
use super::socket::{failure, success};
use super::state::ProductRouteState;
use crate::installed_apps::application::{
    AppsCaller, AppsError, Checked, Installed, IssuedAppCredential, Review,
};
use nessa_app_package::{InlineAsset, InlinePackage};
use nessa_auth::application::app_credential::AppCapability as GrantedCapability;
use nessa_auth::application::session::AuthenticatedSession;
use nessa_protocol::product::generated::{
    AppCapability, AppPackage, AppPackageAsset, AppReview, AppsErrorCode, AppsInstallParams,
    AppsInstallResult, AppsInstalledResult, AppsIssueCredentialParams, AppsIssueCredentialResult,
    AppsPackageResult, AppsReadParams, AppsReviewParams, AppsRevokeCredentialsParams,
    AppsRevokeCredentialsResult, AppsStarterParams, AppsUninstallParams, AppsUninstallResult,
    InstalledApp,
};
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};
use nessa_types::app_identity::ExtensionAppId;
use serde::de::DeserializeOwned;
use serde_json::json;

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    let Some(apps) = state.accounts.apps.as_ref() else {
        return failure(&frame.id, AppsErrorCode::AppsNotConfigured.as_str());
    };
    let id = frame.id.clone();
    match frame.method.as_str() {
        "apps.starter" => {
            let Some(params) = decode::<AppsStarterParams>(&frame) else {
                return failure(&id, "invalid_request");
            };
            match apps.starter(params.app_id, params.package_version) {
                Ok(checked) => success(&id, &package_result(checked)),
                Err(error) => failure(&id, code(error).as_str()),
            }
        }
        "apps.review" => {
            let Some(params) = decode::<AppsReviewParams>(&frame) else {
                return failure(&id, "invalid_request");
            };
            match apps.review(&inline(params.package)) {
                Ok(review) => success(&id, &wire_review(review)),
                Err(error) => failure(&id, code(error).as_str()),
            }
        }
        "apps.install" => {
            let Some(params) = decode::<AppsInstallParams>(&frame) else {
                return failure(&id, "invalid_request");
            };
            let package = inline(params.package);
            match apps
                .install(
                    caller(session),
                    params.request_id.clone(),
                    &package,
                    &params.reviewed_digest,
                )
                .await
            {
                Ok(app) => success(
                    &id,
                    &AppsInstallResult {
                        request_id: params.request_id,
                        app: wire_app(app),
                    },
                ),
                Err(error) => failure(&id, code(error).as_str()),
            }
        }
        "apps.installed" => {
            if !(frame.params.is_null() || frame.params == json!({})) {
                return failure(&id, "invalid_request");
            }
            match apps.installed().await {
                Ok(listed) => success(
                    &id,
                    &AppsInstalledResult {
                        apps: listed.into_iter().map(wire_app).collect(),
                    },
                ),
                Err(error) => failure(&id, code(error).as_str()),
            }
        }
        "apps.read" => {
            let Some(app) =
                decode::<AppsReadParams>(&frame).and_then(|p| ExtensionAppId::new(p.app_id).ok())
            else {
                return failure(&id, "invalid_request");
            };
            match apps.read(app).await {
                Ok(checked) => success(&id, &package_result(checked)),
                Err(error) => failure(&id, code(error).as_str()),
            }
        }
        "apps.uninstall" => {
            let Some((params, app)) = decode::<AppsUninstallParams>(&frame).and_then(|p| {
                let app = ExtensionAppId::new(p.app_id.clone()).ok()?;
                Some((p, app))
            }) else {
                return failure(&id, "invalid_request");
            };
            match apps
                .uninstall(caller(session), params.request_id.clone(), app)
                .await
            {
                Ok(app_id) => success(
                    &id,
                    &AppsUninstallResult {
                        request_id: params.request_id,
                        app_id,
                    },
                ),
                Err(error) => failure(&id, code(error).as_str()),
            }
        }
        "apps.issueCredential" => {
            let Some((params, app)) = decode::<AppsIssueCredentialParams>(&frame).and_then(|p| {
                let app = ExtensionAppId::new(p.app_id.clone()).ok()?;
                Some((p, app))
            }) else {
                return failure(&id, "invalid_request");
            };
            match apps
                .issue_credential(session, caller(session), params.request_id.clone(), app)
                .await
            {
                Ok(issued) => success(&id, &issued_result(params, issued)),
                Err(error) => failure(&id, code(error).as_str()),
            }
        }
        "apps.revokeCredentials" => {
            let Some((params, app)) = decode::<AppsRevokeCredentialsParams>(&frame).and_then(|p| {
                let app = ExtensionAppId::new(p.app_id.clone()).ok()?;
                Some((p, app))
            }) else {
                return failure(&id, "invalid_request");
            };
            match apps
                .revoke_credentials(caller(session), params.request_id.clone(), app)
                .await
            {
                Ok(revoked) => success(
                    &id,
                    &AppsRevokeCredentialsResult {
                        request_id: params.request_id,
                        app_id: params.app_id,
                        revoked: revoked as u64,
                    },
                ),
                Err(error) => failure(&id, code(error).as_str()),
            }
        }
        _ => failure(&id, "unknown_method"),
    }
}

fn issued_result(
    params: AppsIssueCredentialParams,
    issued: IssuedAppCredential,
) -> AppsIssueCredentialResult {
    AppsIssueCredentialResult {
        request_id: params.request_id,
        app_id: params.app_id,
        credential_id: issued.credential_id,
        credential: issued.secret,
        expires_at: issued.expires_at,
        capabilities: issued
            .capabilities
            .into_iter()
            .map(wire_capability)
            .collect(),
    }
}

/// A capability as the wire names it.
pub(super) fn wire_capability(capability: GrantedCapability) -> AppCapability {
    match capability {
        GrantedCapability::List => AppCapability::List,
        GrantedCapability::Read => AppCapability::Read,
        GrantedCapability::Create => AppCapability::Create,
        GrantedCapability::Subscribe => AppCapability::Subscribe,
        GrantedCapability::Stop => AppCapability::Stop,
        GrantedCapability::Send => AppCapability::Send,
    }
}

fn decode<T: DeserializeOwned>(frame: &RequestFrame) -> Option<T> {
    serde_json::from_value(frame.params.clone()).ok()
}

fn caller(session: &AuthenticatedSession) -> AppsCaller {
    let context = session.context();
    AppsCaller {
        organization_id: context.organization_id().as_str().to_owned(),
        principal_id: context.principal_id().as_str().to_owned(),
        credential_id: context.credential_id().as_str().to_owned(),
    }
}

fn inline(package: AppPackage) -> InlinePackage {
    InlinePackage {
        manifest_base64: package.manifest,
        assets: package
            .assets
            .into_iter()
            .map(|asset| InlineAsset {
                path: asset.path,
                bytes_base64: asset.bytes,
            })
            .collect(),
    }
}

fn wire_package(package: InlinePackage) -> AppPackage {
    AppPackage {
        manifest: package.manifest_base64,
        assets: package
            .assets
            .into_iter()
            .map(|asset| AppPackageAsset {
                path: asset.path,
                bytes: asset.bytes_base64,
            })
            .collect(),
    }
}

fn wire_review(review: Review) -> AppReview {
    AppReview {
        app_id: review.app_id,
        package_version: review.package_version,
        entrypoint: review.entrypoint,
        asset_count: review.asset_count as u64,
        digest: review.digest,
        requested_capabilities: review
            .requested_capabilities
            .into_iter()
            .map(wire_capability)
            .collect(),
    }
}

fn package_result(checked: Checked) -> AppsPackageResult {
    AppsPackageResult {
        package: wire_package(checked.package),
        review: wire_review(checked.review),
    }
}

fn wire_app(app: Installed) -> InstalledApp {
    InstalledApp {
        app_id: app.app_id,
        package_version: app.package_version,
    }
}

fn code(error: AppsError) -> AppsErrorCode {
    match error {
        AppsError::PackageInvalid => AppsErrorCode::AppPackageInvalid,
        AppsError::PackageTooLarge => AppsErrorCode::AppPackageTooLarge,
        AppsError::ReviewStale => AppsErrorCode::AppReviewStale,
        AppsError::AlreadyInstalled => AppsErrorCode::AppAlreadyInstalled,
        AppsError::LimitReached => AppsErrorCode::AppsLimitReached,
        AppsError::NotInstalled => AppsErrorCode::AppNotInstalled,
        AppsError::StorageUnavailable => AppsErrorCode::AppsStorageUnavailable,
        AppsError::AuditUnavailable => AppsErrorCode::AppsAuditUnavailable,
        AppsError::RequestsNothing => AppsErrorCode::AppRequestsNothing,
        AppsError::CredentialConflict => AppsErrorCode::AppCredentialConflict,
        AppsError::CredentialsUnavailable => AppsErrorCode::AppCredentialsUnavailable,
    }
}
