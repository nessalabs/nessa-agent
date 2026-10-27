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
pub(super) fn store(
    environment: &Environment,
    vault: Arc<nessa_vault::application::Vault>,
) -> Result<Arc<SqliteRelayStore>, Failure> {
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
        Arc::new(crate::github_relay::infrastructure::VaultInboxKey::new(
            root.join("github-relay/inbox.sqlite"),
            Arc::new(crate::secrets::VaultSecret::new(
                vault,
                nessa_vault::domain::EntryName::new("github-inbox-key")
                    .expect("static vault entry"),
            )),
        )),
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
pub(super) async fn connect(
    repository: String,
    accept_gap: bool,
    passphrase_stdin: bool,
) -> Result<(), RunError> {
    let environment = Environment::from_system()?;
    let (vault, _session) = cli_vault(&environment, passphrase_stdin).await?;
    let result=async {
        let store=store(&environment,vault.vault.clone())?;
        let source=source(&environment)?;
        let credentials=credentials(&environment,vault.vault.clone()).await?;
        let binding=credentials.identify(&repository).await?;
        store.bind(&binding,accept_gap).await?;
        let token=credentials.token().await?;
        catch_up(source.as_ref(),store.as_ref(),&binding,&token).await?;
        Ok::<_,Failure>(serde_json::json!({"result":"connected","repository":binding.repository_name(),"repositoryId":binding.repository_id(),"userId":binding.user_id(),"stage":environment.stage.as_str()}))
    }.await.map_err(|error|RunError::Runtime(error.to_string()))?;
    output(result)
}
pub(super) async fn events(repository: String, passphrase_stdin: bool) -> Result<(), RunError> {
    let environment = Environment::from_system()?;
    let (vault, _session) = cli_vault(&environment, passphrase_stdin).await?;
    let result=async {
        let store=store(&environment,vault.vault.clone())?;let mut events=Vec::new();
        for binding in store.bindings().await? {
            if binding.repository_name()!=repository { continue; }
            for event in store.events(&binding).await? { events.push(serde_json::json!({"userId":binding.user_id(),"repositoryId":event.repository_id,"sequence":event.sequence,"deliveryId":event.delivery_id,"event":event.event,"payload":serde_json::from_str::<serde_json::Value>(&event.payload).map_err(|_|Failure::InvalidResponse)?,"receivedAt":event.received_at})); }
        }
        Ok::<_,Failure>(serde_json::json!({"events":events,"stage":environment.stage.as_str()}))
    }.await.map_err(|error|RunError::Runtime(error.to_string()))?;
    output(result)
}
pub(super) async fn disconnect(repository: String, passphrase_stdin: bool) -> Result<(), RunError> {
    let environment = Environment::from_system()?;
    let (vault, _session) = cli_vault(&environment, passphrase_stdin).await?;
    let result=async {
        let store=store(&environment,vault.vault.clone())?;let mut removed=0;
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
    vault: Arc<nessa_vault::application::Vault>,
) -> Result<Arc<dyn crate::github_relay::application::ConnectedGithub>, Failure> {
    let namespace = environment
        .auth_directory
        .as_ref()
        .and_then(|p| p.parent())
        .ok_or(Failure::Storage)?;
    let github = super::github::compose(namespace, environment, vault)
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
    _vault: Arc<nessa_vault::application::Vault>,
) -> Result<Arc<dyn crate::github_relay::application::ConnectedGithub>, Failure> {
    Err(Failure::CredentialsUnavailable)
}

async fn cli_vault(
    environment: &Environment,
    passphrase_stdin: bool,
) -> Result<
    (
        Arc<crate::product::VaultRoutes>,
        nessa_vault::application::Session,
    ),
    RunError,
> {
    if !passphrase_stdin {
        return Err(RunError::Runtime("vault locked; create it in Nessa, then use --passphrase-stdin for local GitHub commands".into()));
    }
    let root = environment
        .auth_directory
        .as_ref()
        .and_then(|p| p.parent())
        .ok_or_else(|| RunError::Runtime("vault namespace unavailable".into()))?;
    let routes = super::vault::compose(root);
    let opened = routes.clone();
    tokio::task::spawn_blocking(move || {
        use std::io::{BufRead, Read};
        let mut input = zeroize::Zeroizing::new(Vec::new());
        std::io::stdin()
            .lock()
            .take((nessa_vault::contract::MAXIMUM_PASSPHRASE_BYTES + 2) as u64)
            .read_until(b'\n', &mut input)
            .map_err(|_| "vault input unavailable")?;
        if input.last() == Some(&b'\n') {
            input.pop();
            if input.last() == Some(&b'\r') {
                input.pop();
            }
        }
        let phrase = nessa_vault::domain::Passphrase::new(
            String::from_utf8(std::mem::take(&mut *input)).map_err(|_| "invalid vault input")?,
        )
        .map_err(|_| "invalid vault passphrase")?;
        let session = opened.vault.session(
            uuid::Uuid::new_v4().to_string(),
            nessa_vault::application::Caller {
                principal: format!(
                    "local-account:{}",
                    super::install_command::local_account_id()
                ),
                credential: "local-filesystem-and-passphrase".into(),
            },
        );
        opened
            .vault
            .unlock(&session.lease(), phrase)
            .map_err(|_| "vault could not be unlocked")?;
        let snapshot = opened
            .migration
            .capture()
            .map_err(|_| "existing data migration unavailable")?;
        crate::vault::application::verify_and_retire(&opened.vault, snapshot.as_ref())
            .map_err(|_| "existing data migration unavailable")?;
        Ok::<_, &'static str>((routes, session))
    })
    .await
    .map_err(|_| RunError::Runtime("vault worker unavailable".into()))?
    .map_err(|message| RunError::Runtime(message.into()))
}
