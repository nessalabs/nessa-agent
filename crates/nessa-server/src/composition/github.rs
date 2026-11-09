//! The GitHub sign-in, composed once per run: the Nessa GitHub App, GitHub
//! over HTTPS, its encrypted app-session vault entry and audit, and the gateway
//! broker and live-event relay that use it.
//!
//! `NESSA_GITHUB_APP` names a file describing another app in place of the
//! released one, so the stress check can point a gateway at a fake GitHub on
//! this machine. What such a file may say is [`GithubApp::parse`]'s; this
//! only reads it.
use std::{path::Path, sync::Arc};

use super::local_auth::{conversation_root, SystemClock};
use crate::github::application::GithubConnections;
use crate::github::infrastructure::{DurableGithubAudit, GithubApp, GithubHttps, GithubTokenVault};
use crate::product::GithubRoutes;
use crate::timer::TokioTimer;

/// The routes the GitHub methods answer from.
pub(super) struct GithubComposition {
    pub(super) routes: Arc<GithubRoutes>,
    pub(super) connections: Arc<GithubConnections>,
}

/// The sign-in kept under `namespace`, or `None` — logged — when the app
/// override cannot be read or no HTTP client can be built. Without it the
/// github methods answer `github_not_configured` and agents are given no
/// GitHub sign-in.
pub(super) async fn compose(
    namespace: &Path,
    environment: &crate::env::Environment,
    vault: Arc<nessa_vault::application::Vault>,
) -> Option<GithubComposition> {
    if crate::github::infrastructure::remove_plaintext_sign_in(namespace.join("github"))
        .await
        .is_err()
    {
        tracing::error!("a plaintext GitHub sign-in could not be removed");
        return None;
    }
    let app = match std::env::var_os("NESSA_GITHUB_APP") {
        None => GithubApp::released(),
        Some(path) => match std::fs::read(&path).map(|bytes| GithubApp::parse(&bytes)) {
            Ok(Ok(app)) => app,
            Ok(Err(problem)) => {
                tracing::error!(
                    ?problem,
                    "GitHub sign-in is off this run: NESSA_GITHUB_APP does not describe an app"
                );
                return None;
            }
            Err(error) => {
                tracing::error!(%error, "GitHub sign-in is off this run: NESSA_GITHUB_APP could not be read");
                return None;
            }
        },
    };
    let install_url = app.install_url();
    let Some(api) = GithubHttps::new(app.clone()) else {
        tracing::error!("GitHub sign-in is off this run: no HTTPS client could be built");
        return None;
    };
    let secret = crate::secrets::VaultSecret::new(
        vault.clone(),
        nessa_vault::domain::EntryName::new("github-sign-in").expect("static vault entry"),
    );
    let connections = Arc::new(
        GithubConnections::load(
            Arc::new(api),
            Arc::new(crate::github::infrastructure::GithubVaultAccess(
                vault.clone(),
            )),
            Arc::new(GithubTokenVault::new(
                namespace.join("github"),
                Arc::new(secret),
            )),
            Arc::new(DurableGithubAudit::new(
                conversation_root(namespace).join("audit").join("github"),
                Arc::new(SystemClock),
            )),
            Arc::new(TokioTimer::new()),
        )
        .await,
    );
    let live = if crate::github_relay::infrastructure::relay_configuration(environment.stage)
        .is_ok_and(|relay| relay.client_id == app.client_id && relay.app_slug == app.slug)
    {
        match (
            super::github_relay::http(),
            super::github_relay::store(environment, vault),
        ) {
            (Ok(http), Ok(store)) => {
                Some(Arc::new(crate::github_relay::application::LiveEvents::new(
                    Arc::new(
                        crate::github_relay::infrastructure::ConnectedGithubCredentials::new(
                            http,
                            connections.clone(),
                            app,
                        ),
                    ),
                    store,
                )))
            }
            _ => None,
        }
    } else {
        None
    };
    Some(GithubComposition {
        routes: Arc::new(GithubRoutes {
            connections: connections.clone(),
            install_url,
            live,
            changes: tokio::sync::Mutex::new(()),
        }),
        connections,
    })
}
