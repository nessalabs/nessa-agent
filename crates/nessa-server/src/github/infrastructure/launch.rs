//! What an agent is given of the GitHub sign-in at each provider open.
//!
//! `GH_TOKEN` for the `gh` command, and a git credential helper for
//! `https://github.com`, passed as `GIT_CONFIG_*` so no file of the person's
//! is touched. The helper list for that URL is emptied first (an empty value
//! resets it), so a stale credential kept by another helper, such as the
//! macOS keychain's, is not tried before this one. Remotes over SSH are left
//! to the person's own keys. Signed out, an agent is given nothing and keeps
//! whatever sign-in its machine has.
use std::{collections::BTreeMap, ffi::OsString, sync::Arc};

use nessa_sdk::infrastructure::acp::sessions::OpenCredentialSource;

use crate::github::application::GithubConnections;
use crate::github::domain::AccessToken;

const HELPER_KEY: &str = "credential.https://github.com.helper";
/// Answers `get` with the token, read from the agent's own environment so it
/// is never written into git's configuration. A process whose `GH_TOKEN` was
/// taken away (Codex drops `*TOKEN*` from some shells) answers nothing rather
/// than an empty password.
const HELPER: &str = "!f() { test \"$1\" = get && test -n \"$GH_TOKEN\" && echo username=x-access-token && echo \"password=$GH_TOKEN\"; }; f";

/// The variables an agent is given for `token`.
pub fn variables(token: &AccessToken) -> BTreeMap<OsString, OsString> {
    [
        ("GH_TOKEN", token.expose()),
        ("GIT_CONFIG_COUNT", "2"),
        ("GIT_CONFIG_KEY_0", HELPER_KEY),
        ("GIT_CONFIG_VALUE_0", ""),
        ("GIT_CONFIG_KEY_1", HELPER_KEY),
        ("GIT_CONFIG_VALUE_1", HELPER),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.into()))
    .collect()
}

/// The sign-in as each provider open reads it.
pub struct GithubLaunchCredentials {
    connections: Arc<GithubConnections>,
}

impl GithubLaunchCredentials {
    pub fn new(connections: Arc<GithubConnections>) -> Self {
        Self { connections }
    }
}

impl OpenCredentialSource for GithubLaunchCredentials {
    fn variables(&self) -> BTreeMap<OsString, OsString> {
        self.connections
            .token()
            .map(|token| variables(&token))
            .unwrap_or_default()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// What git's credential helpers answer for https://github.com with
    /// `given` alone: no prompt, no system or user configuration.
    fn fill(given: &BTreeMap<OsString, OsString>) -> String {
        let home = tempfile::tempdir().unwrap();
        let output = std::process::Command::new("git")
            .args(["credential", "fill"])
            .env_clear()
            .envs(given)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", home.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(b"protocol=https\nhost=github.com\n\n")?;
                child.wait_with_output()
            })
            .expect("git runs");
        String::from_utf8(output.stdout).unwrap()
    }

    /// The helper really answers git: run it the way git does, through the
    /// shell, with the variables an agent is given.
    #[test]
    fn git_reads_the_token_through_the_helper() {
        let mut given = variables(&AccessToken::new("ghu_fixture".into()).unwrap());
        let answer = fill(&given);
        assert!(answer.contains("username=x-access-token\n"), "{answer}");
        assert!(answer.contains("password=ghu_fixture\n"), "{answer}");
        // A shell that dropped the token is given no empty password.
        given.remove(&OsString::from("GH_TOKEN"));
        let answer = fill(&given);
        assert!(!answer.contains("password="), "{answer}");
    }
}
