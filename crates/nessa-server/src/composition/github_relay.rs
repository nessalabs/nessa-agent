//! Optional provider inbox. The host supplies its baked stage through the existing
//! managed launch contract; absent stage configuration leaves this feature off.
use crate::{
    core::RunError,
    env::Environment,
    github_relay::{
        application::{catch_up, LiveEvents, RelayStore},
        domain::Failure,
        infrastructure::{
            relay_configuration, ConnectedRelayStore, HttpRelaySource, SqliteRelayStore,
        },
    },
};
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::task::JoinHandle;

pub(super) fn http() -> Result<reqwest::Client, Failure> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .user_agent("Nessa-GitHub-relay")
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| Failure::Offline)
}
pub(super) fn store(environment: &Environment) -> Result<Arc<SqliteRelayStore>, Failure> {
    let root = environment
        .auth_directory
        .as_ref()
        .and_then(|p| p.parent())
        .ok_or(Failure::Storage)?;
    Ok(Arc::new(SqliteRelayStore::new(
        root.join("github-relay/inbox.sqlite"),
        Arc::new(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        }),
    )))
}
fn source(environment: &Environment) -> Result<Arc<HttpRelaySource>, Failure> {
    Ok(Arc::new(HttpRelaySource::new(
        http()?,
        relay_configuration(environment.stage)?,
        environment.stage,
    )?))
}
pub(super) struct RelayRuntime(JoinHandle<()>);
impl Drop for RelayRuntime {
    fn drop(&mut self) {
        self.0.abort();
    }
}
pub(super) fn start(environment: &Environment, live: Arc<LiveEvents>) -> Option<RelayRuntime> {
    let source = match source(environment) {
        Ok(source) => source,
        Err(Failure::UnconfiguredStage) => return None,
        Err(error) => {
            tracing::warn!(%error,"GitHub relay disabled");
            return None;
        }
    };
    let store = Arc::new(ConnectedRelayStore::new(
        live.store.clone(),
        live.credentials.clone(),
    ));
    let credentials = live.credentials.clone();
    Some(RelayRuntime(tokio::spawn(
        crate::github_relay::application::serve(
            source,
            store,
            credentials,
            Arc::new(crate::github_relay::infrastructure::TokioRelayWait),
            Arc::new(crate::github_relay::infrastructure::TracingRelayReporter),
        ),
    )))
}
pub(super) async fn connect(repository: String, accept_gap: bool) -> Result<(), RunError> {
    let environment = Environment::from_system()?;
    let result=async {
        let store=store(&environment)?;
        let source=source(&environment)?;
        let credentials=credentials(&environment).await?;
        let binding=credentials.identify(&repository).await?;
        store.bind(&binding,accept_gap).await?;
        let token=credentials.token().await?;
        catch_up(source.as_ref(),store.as_ref(),&binding,&token).await?;
        Ok::<_,Failure>(serde_json::json!({"result":"connected","repository":binding.repository_name(),"repositoryId":binding.repository_id(),"userId":binding.user_id(),"stage":environment.stage.as_str()}))
    }.await.map_err(|error|RunError::Runtime(error.to_string()))?;
    output(result)
}
pub(super) async fn events(repository: String) -> Result<(), RunError> {
    let environment = Environment::from_system()?;
    let result=async {
        let store=store(&environment)?;let mut events=Vec::new();
        for binding in store.bindings().await? {
            if binding.repository_name()!=repository { continue; }
            for event in store.events(&binding).await? { events.push(serde_json::json!({"userId":binding.user_id(),"repositoryId":event.repository_id,"sequence":event.sequence,"deliveryId":event.delivery_id,"event":event.event,"payload":serde_json::from_str::<serde_json::Value>(&event.payload).map_err(|_|Failure::InvalidResponse)?,"receivedAt":event.received_at})); }
        }
        Ok::<_,Failure>(serde_json::json!({"events":events,"stage":environment.stage.as_str()}))
    }.await.map_err(|error|RunError::Runtime(error.to_string()))?;
    output(result)
}
pub(super) async fn disconnect(repository: String) -> Result<(), RunError> {
    let environment = Environment::from_system()?;
    let result=async {
        let store=store(&environment)?;let mut removed=0;
        for binding in store.bindings().await? {
            if binding.repository_name()==repository { store.unbind(&binding).await?;removed+=1; }
        }
        Ok::<_,Failure>(serde_json::json!({"result":"disconnected","bindingsRemoved":removed,"stage":environment.stage.as_str()}))
    }.await.map_err(|error|RunError::Runtime(error.to_string()))?;
    output(result)
}
fn output(result: serde_json::Value) -> Result<(), RunError> {
    use std::io::Write;
    let mut bytes =
        serde_json::to_vec(&result).map_err(|error| RunError::Runtime(error.to_string()))?;
    bytes.push(b'\n');
    std::io::stdout().write_all(&bytes)?;
    Ok(())
}

#[cfg(unix)]
async fn credentials(
    environment: &Environment,
) -> Result<Arc<dyn crate::github_relay::application::ConnectedGithub>, Failure> {
    let namespace = environment
        .auth_directory
        .as_ref()
        .and_then(|p| p.parent())
        .ok_or(Failure::Storage)?;
    let github = super::github::compose(namespace, environment)
        .await
        .ok_or(Failure::CredentialsUnavailable)?;
    Ok(github
        .routes
        .live
        .as_ref()
        .ok_or(Failure::UnconfiguredStage)?
        .credentials
        .clone())
}

// Agent-backed GitHub sign-in is composed only on Unix in mainline. Other
// targets retain local inbox reads and refuse connecting without that owner.
#[cfg(not(unix))]
async fn credentials(
    _environment: &Environment,
) -> Result<Arc<dyn crate::github_relay::application::ConnectedGithub>, Failure> {
    Err(Failure::CredentialsUnavailable)
}
