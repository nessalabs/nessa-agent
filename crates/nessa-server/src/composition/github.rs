//! The GitHub sign-in, composed once per run: the Nessa GitHub App, GitHub
//! over HTTPS, the private sign-in file, its audit, and what each agent open
//! reads of it.
//!
//! `NESSA_GITHUB_APP` names a file describing another app in place of the
//! released one, so the stress check can point a gateway at a fake GitHub on
//! this machine. What such a file may say is [`GithubApp::parse`]'s; this
//! only reads it.
use std::{path::Path, sync::Arc};

use super::local_auth::{conversation_root, SystemClock};
use crate::github::application::GithubConnections;
use crate::github::infrastructure::{DurableGithubAudit, GithubApp, GithubHttps, GithubTokenFile};
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
pub(super) async fn compose(namespace: &Path) -> Option<GithubComposition> {
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
    let Some(api) = GithubHttps::new(app) else {
        tracing::error!("GitHub sign-in is off this run: no HTTPS client could be built");
        return None;
    };
    let connections = Arc::new(
        GithubConnections::load(
            Arc::new(api),
            Arc::new(GithubTokenFile::new(namespace.join("github"))),
            Arc::new(DurableGithubAudit::new(
                conversation_root(namespace).join("audit").join("github"),
                Arc::new(SystemClock),
            )),
            Arc::new(TokioTimer::new()),
        )
        .await,
    );
    Some(GithubComposition {
        routes: Arc::new(GithubRoutes {
            connections: connections.clone(),
            install_url,
        }),
        connections,
    })
}
