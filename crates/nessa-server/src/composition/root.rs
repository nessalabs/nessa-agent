use crate::app::state::AppState;
use crate::core::RunError;
use crate::env::{Environment, Stage};
use crate::server::entrypoint::http;

pub struct CompositionRoot;

impl CompositionRoot {
    pub async fn serve() -> Result<(), RunError> {
        let config = Environment::from_system()?;
        warn_dev_stage(&config);

        let state = AppState::from_environment(&config);
        let router = http::router(state);

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

fn warn_dev_stage(config: &Environment) {
    if config.stage == Stage::Dev {
        tracing::warn!(
            stage = config.stage.as_str(),
            "dev stage allows default auth credential when NESSA_TOKEN is unset",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{MockEnv, STAGE, TOKEN};

    #[test]
    fn environment_loads_for_ci_stage() {
        let config = Environment::load(
            &MockEnv::new()
                .set(STAGE, "ci")
                .set(TOKEN, "test"),
        )
        .expect("ci config");
        assert_eq!(config.stage, Stage::Ci);
    }
}
