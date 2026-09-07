use crate::core::RunError;
use crate::env::Environment;
use crate::server::entrypoint::http;

pub struct CompositionRoot;

impl CompositionRoot {
    /// Serve normally, or execute an explicit offline credential command.
    pub async fn run(args: &[String]) -> Result<(), RunError> {
        if args.is_empty() {
            Self::serve().await
        } else {
            super::auth_command::execute(args)
        }
    }

    pub async fn serve() -> Result<(), RunError> {
        let config = Environment::from_system()?;
        let dependencies = runtime_dependencies(&config);
        let product = super::local_auth::product_state(&config, dependencies.clock.clone())?;
        let router = http::router(product);

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
            "nessa-server listening",
        );

        axum::serve(listener, router).await?;
        Ok(())
    }
}

fn runtime_dependencies(config: &Environment) -> crate::app::dependencies::RuntimeDependencies {
    use crate::{app::dependencies::RuntimeDependencies, env::UptimeBackend};
    match config.uptime_backend {
        UptimeBackend::Monotonic => RuntimeDependencies::default(),
        UptimeBackend::Fixed(ms) => RuntimeDependencies::fixed_uptime(ms),
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
