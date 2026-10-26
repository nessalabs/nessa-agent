//! Reads the existing GitHub sign-in owner. No separate CLI credential,
//! client secret, or token copy on disk belongs to the relay.
use crate::github::{
    application::GithubConnections, domain::GithubStatus, infrastructure::GithubApp,
};
use crate::github_relay::{
    application::{ConnectedGithub, GitHubCredentials},
    domain::{Binding, Failure},
};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::sync::Arc;
pub struct ConnectedGithubCredentials {
    http: reqwest::Client,
    connections: Arc<GithubConnections>,
    app: GithubApp,
}
impl ConnectedGithubCredentials {
    pub fn new(http: reqwest::Client, connections: Arc<GithubConnections>, app: GithubApp) -> Self {
        Self {
            http,
            connections,
            app,
        }
    }
    async fn get<T: DeserializeOwned>(&self, path: &str, token: &str) -> Result<T, Failure> {
        let mut response = self
            .http
            .get(format!("{}{path}", self.app.api_url))
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|_| Failure::Offline)?;
        match response.status().as_u16() {
            401 => return Err(Failure::CredentialsUnavailable),
            403 | 404 => return Err(Failure::Forbidden),
            200 => (),
            _ => return Err(Failure::Offline),
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Failure::Offline)? {
            if bytes.len() + chunk.len() > 1_048_576 {
                return Err(Failure::InvalidResponse);
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| Failure::InvalidResponse)
    }
    fn binding(&self, user: &str, repo: Repository) -> Result<Binding, Failure> {
        Binding::new(
            user.to_owned(),
            repo.id.to_string(),
            repo.full_name,
            uuid::Uuid::new_v4(),
        )
    }
}
#[derive(Deserialize)]
struct Repository {
    id: i64,
    full_name: String,
}
#[derive(Deserialize)]
struct Installations {
    total_count: usize,
    installations: Vec<Installation>,
}
#[derive(Deserialize)]
struct Installation {
    id: i64,
    app_slug: String,
    suspended_at: Option<String>,
}
#[derive(Deserialize)]
struct Repositories {
    total_count: usize,
    repositories: Vec<Repository>,
}
#[async_trait::async_trait]
impl GitHubCredentials for ConnectedGithubCredentials {
    async fn token(&self) -> Result<String, Failure> {
        self.connections
            .token()
            .map(|t| t.expose().to_owned())
            .ok_or(Failure::CredentialsUnavailable)
    }
    async fn identify(&self, repository: &str) -> Result<Binding, Failure> {
        if !Binding::repository_name_valid(repository) {
            return Err(Failure::InvalidResponse);
        }
        let user = self.user_id().ok_or(Failure::CredentialsUnavailable)?;
        let token = self.token().await?;
        let repo = self.get(&format!("repos/{repository}"), &token).await?;
        if self.user_id().as_deref() != Some(&user) {
            return Err(Failure::IdentityChanged);
        }
        self.binding(&user, repo)
    }
}
#[async_trait::async_trait]
impl ConnectedGithub for ConnectedGithubCredentials {
    fn user_id(&self) -> Option<String> {
        match self.connections.status() {
            GithubStatus::Connected { account, .. } => Some(account.id.to_string()),
            _ => None,
        }
    }
    async fn repositories(&self) -> Result<Vec<Binding>, Failure> {
        let user = self.user_id().ok_or(Failure::CredentialsUnavailable)?;
        let token = self.token().await?;
        let installations: Installations =
            self.get("user/installations?per_page=100", &token).await?;
        if installations.total_count != installations.installations.len() {
            return Err(Failure::Capacity);
        }
        let mut bindings = Vec::new();
        for install in installations.installations {
            if install.app_slug != self.app.slug || install.suspended_at.is_some() {
                continue;
            }
            let repos: Repositories = self
                .get(
                    &format!(
                        "user/installations/{}/repositories?per_page=100",
                        install.id
                    ),
                    &token,
                )
                .await?;
            if repos.total_count != repos.repositories.len() {
                return Err(Failure::Capacity);
            }
            for repo in repos.repositories {
                if !bindings
                    .iter()
                    .any(|b: &Binding| b.repository_id() == repo.id.to_string())
                {
                    bindings.push(self.binding(&user, repo)?);
                }
                if bindings.len() > crate::github_relay::domain::contract::BINDINGS as usize {
                    return Err(Failure::Capacity);
                }
            }
        }
        Ok(bindings)
    }
}
