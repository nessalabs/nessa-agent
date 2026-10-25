use crate::github_relay::{
    application::GitHubCredentials,
    domain::{Binding, Failure},
};
use serde::Deserialize;
use std::time::Duration;
pub struct GhCredentials {
    http: reqwest::Client,
}
impl GhCredentials {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }
}
#[derive(Deserialize)]
struct Identity {
    id: i64,
}
#[async_trait::async_trait]
impl GitHubCredentials for GhCredentials {
    async fn token(&self) -> Result<String, Failure> {
        let output = tokio::time::timeout(
            Duration::from_secs(10),
            tokio::process::Command::new("gh")
                .args(["auth", "token", "--hostname", "github.com"])
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| Failure::CredentialsUnavailable)?
        .map_err(|_| Failure::CredentialsUnavailable)?;
        if !output.status.success() || output.stdout.len() > 256 {
            return Err(Failure::CredentialsUnavailable);
        }
        let value =
            String::from_utf8(output.stdout).map_err(|_| Failure::CredentialsUnavailable)?;
        let value = value.trim();
        if value.len() < 10
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(Failure::CredentialsUnavailable);
        }
        Ok(value.to_owned())
    }
    async fn identify(&self, repository: &str) -> Result<Binding, Failure> {
        if !Binding::repository_name_valid(repository) {
            return Err(Failure::InvalidResponse);
        }
        let token = self.token().await?;
        let user = self
            .http
            .get("https://api.github.com/user")
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|_| Failure::Offline)?;
        if !user.status().is_success() {
            return Err(Failure::CredentialsUnavailable);
        }
        let user: Identity = user.json().await.map_err(|_| Failure::InvalidResponse)?;
        let repo = self
            .http
            .get(format!("https://api.github.com/repos/{repository}"))
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|_| Failure::Offline)?;
        if !repo.status().is_success() {
            return Err(Failure::Forbidden);
        }
        let repo: Identity = repo.json().await.map_err(|_| Failure::InvalidResponse)?;
        if user.id == 0 || repo.id == 0 {
            return Err(Failure::InvalidResponse);
        }
        Binding::new(
            user.id.to_string(),
            repo.id.to_string(),
            repository.to_owned(),
            uuid::Uuid::new_v4(),
        )
    }
}
