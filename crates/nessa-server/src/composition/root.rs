use crate::cli::entrypoint::{parse, Command, HELP};
use crate::desktop_runtime::{
    application::{restore_retirement, retire},
    infrastructure::RetirementFiles,
};
use crate::env::Environment;
use crate::server::entrypoint::http;
use crate::{app::dependencies::RuntimeDependencies, core::RunError, env::UptimeBackend};
use axum::Extension;
use std::io::Write;
#[cfg(unix)]
use tokio::signal::unix::{signal, SignalKind};
use uuid::Uuid;

pub struct CompositionRoot;

impl CompositionRoot {
    /// Dispatch the single CLI contract; local provisioning never contacts a server.
    pub async fn run(args: &[String]) -> Result<(), RunError> {
        match parse(args).map_err(RunError::Authentication)? {
            Command::Help => {
                std::io::stdout().write_all(HELP.as_bytes())?;
                Ok(())
            }
            Command::Server => Self::serve().await,
            Command::Desktop(directory) => Self::serve_desktop(&directory).await,
            Command::Offline(mut args) => {
                if args.get(1).is_some_and(|v| v == "init")
                    && !args.iter().any(|v| v == "--owner-token-file")
                {
                    let directory = Environment::auth_directory_from_system()?.join("surfaces");
                    nessa_local_storage::create_directory(&directory)
                        .map_err(|e| RunError::Authentication(e.to_string()))?;
                    args.splice(
                        2..2,
                        [
                            "--owner-token-file".into(),
                            directory
                                .join("nessa-cli.token")
                                .to_string_lossy()
                                .into_owned(),
                        ],
                    );
                }
                super::auth_command::execute(&args)
            }
            Command::Token {
                credential_file,
                ttl_seconds,
            } => super::cli::online(true, credential_file, ttl_seconds),
            Command::Doctor { credential_file } => super::cli::online(false, credential_file, None),
        }
    }

    pub async fn serve() -> Result<(), RunError> {
        Self::serve_runtime(None).await
    }

    async fn serve_desktop(bundle: &std::path::Path) -> Result<(), RunError> {
        Self::serve_runtime(Some(bundle)).await
    }

    async fn serve_runtime(bundle: Option<&std::path::Path>) -> Result<(), RunError> {
        let config = Environment::from_system()?;
        if bundle.is_some() {
            super::desktop::prepare(&config)?;
        }
        let dependencies = runtime_dependencies(&config);
        let product =
            super::local_auth::product_state(&config, dependencies.clock.clone(), bundle)?;
        let conversations = product.conversations.clone();
        #[cfg(unix)]
        let retirement_clock = product.clock.clone();
        let desktop_identity = if let Some(bundle) = bundle {
            let configured = std::env::var("NESSA_RUNTIME_FINGERPRINT")
                .map_err(|_| RunError::Agent("missing desktop runtime fingerprint".into()))?;
            let generation = std::env::var("NESSA_SERVICE_GENERATION")
                .map_err(|_| RunError::Agent("missing desktop service generation".into()))?;
            Some(super::desktop::runtime_identity(
                bundle,
                configured,
                generation,
                Uuid::new_v4().to_string(),
                std::process::id(),
            )?)
        } else {
            None
        };
        #[cfg(unix)]
        let retirement_files = if let Some(identity) = &desktop_identity {
            let root = config
                .auth_directory
                .as_ref()
                .and_then(|path| path.parent())
                .ok_or_else(|| RunError::Agent("missing desktop namespace".into()))?;
            let files = RetirementFiles::new(root, retirement_clock).map_err(RunError::Agent)?;
            let fence = files.fence().map_err(RunError::Agent)?;
            restore_retirement(fence.as_ref(), identity, conversations.as_deref())
                .await
                .map_err(RunError::Agent)?;
            Some(files)
        } else {
            None
        };
        let mut router = http::router(product);
        if let Some(identity) = &desktop_identity {
            router = router.layer(Extension(identity.clone()));
        }

        let listen_addr = config.listen_addr();
        let listener = tokio::net::TcpListener::bind(&listen_addr)
            .await
            .map_err(|source| RunError::Bind {
                addr: listen_addr.clone(),
                source,
            })?;

        tracing::info!(
            listen_addr = %listen_addr,
            stage = config.stage.as_str(),
            "nessa server listening",
        );

        #[cfg(unix)]
        if bundle.is_some() {
            if let Some(service) = conversations.clone() {
                let mut requests = signal(SignalKind::user_defined1()).map_err(RunError::Serve)?;
                tokio::spawn(async move {
                    while requests.recv().await.is_some() {
                        if let Err(error) = service.stop_active_agents().await {
                            tracing::error!(%error, "could not stop active agents");
                        }
                    }
                });
            }
        }
        #[cfg(unix)]
        if let Some(identity) = desktop_identity {
            let files = retirement_files.expect("desktop files initialized before admission");
            let service = conversations.clone();
            let mut requests = signal(SignalKind::user_defined2()).map_err(RunError::Serve)?;
            tokio::spawn(async move {
                while requests.recv().await.is_some() {
                    let request = match files.request() {
                        Ok(request) => request,
                        Err(error) => {
                            tracing::error!(%error, "invalid desktop retirement request");
                            continue;
                        }
                    };
                    let result =
                        retire(request, identity.clone(), service.as_deref(), &files).await;
                    if let Err(error) = files.result(&result) {
                        tracing::error!(%error, "could not acknowledge desktop retirement");
                    }
                }
            });
        }
        axum::serve(listener, router).with_graceful_shutdown(async move {
            shutdown_signal().await;
            if let Some(service) = conversations {
                if let Err(error) = service.shutdown().await {
                    tracing::error!(%error, "conversation shutdown did not confirm all cleanup");
                }
            }
        }).await?;
        Ok(())
    }
}

fn runtime_dependencies(config: &Environment) -> RuntimeDependencies {
    match config.uptime_backend {
        UptimeBackend::Monotonic => RuntimeDependencies::default(),
        UptimeBackend::Fixed(ms) => RuntimeDependencies::fixed_uptime(ms),
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{MockEnv, Stage, STAGE};

    #[test]
    fn fixed_clock_flows_from_config_to_runtime() {
        for stage in ["dev", "ci"] {
            let config = Environment::load(
                &MockEnv::new()
                    .set("NESSA_STAGE", stage)
                    .set("NESSA_UPTIME_BACKEND", "fixed")
                    .set("NESSA_UPTIME_FIXED_MS", "123"),
            )
            .unwrap();
            assert_eq!(runtime_dependencies(&config).clock.elapsed_ms(), 123);
        }
    }
    #[test]
    fn environment_loads_for_ci_stage() {
        let config = Environment::load(&MockEnv::new().set(STAGE, "ci")).expect("ci config");
        assert_eq!(config.stage, Stage::Ci);
    }
}
