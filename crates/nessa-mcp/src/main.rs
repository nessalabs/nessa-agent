//! Composition for Nessa's local MCP shell tool. Stdout carries JSON-RPC only.
#[cfg(unix)]
mod mcp;
#[cfg(unix)]
mod shell;

#[cfg(unix)]
use shell::application::ShellService;
#[cfg(unix)]
use shell::infrastructure::{PrivateAudit, ShepherdRunner};
#[cfg(unix)]
use shepherd::SupervisorBuilder;
#[cfg(unix)]
use std::{path::PathBuf, sync::Arc};

#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "MCP shell stopped");
            std::process::ExitCode::FAILURE
        }
    }
}
#[cfg(unix)]
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 4 || args[0] != "--workspace" || args[2] != "--audit-directory" {
        return Err("expected --workspace PATH --audit-directory PATH".into());
    }
    let workspace = PathBuf::from(&args[1]);
    let directory = PathBuf::from(&args[3]);
    if !workspace.is_absolute() || !workspace.is_dir() || !directory.is_absolute() {
        return Err("workspace and audit directory must be absolute; workspace must exist".into());
    }
    let supervisor = SupervisorBuilder::new().build();
    let environment = command_environment(std::env::vars_os());
    let audit = Arc::new(PrivateAudit::new(directory)?);
    let service = Arc::new(ShellService::new(
        Arc::new(ShepherdRunner::new(supervisor.clone(), environment)),
        audit,
    ));
    let result = mcp::serve(service, workspace, uuid::Uuid::new_v4().to_string()).await;
    // Serving and the supervisor's own sweep are separate facts, and a command
    // that stopped badly often fails both. Whichever is returned, the other is
    // not dropped on the way past.
    if let Err(error) = &result {
        tracing::error!(%error, "MCP serving ended with an unreported outcome");
    }
    let cleanup = supervisor.shutdown().await?;
    if !cleanup.scopes.iter().all(|scope| scope.all_verified()) {
        return Err("Shepherd could not verify process cleanup".into());
    }
    result
}

/// What every command is started with, of this server's own environment.
/// Credentials used by the provider are not implicitly inherited by shell
/// commands; the GitHub sign-in the gateway gives this server for its agent
/// (`GH_TOKEN` and git's `GIT_CONFIG_*` credential helper) is meant for them,
/// so `git` and `gh` act as the person.
#[cfg(any(unix, test))]
fn command_environment(
    variables: impl Iterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    const KEPT: [&str; 8] = [
        "PATH", "HOME", "USER", "LOGNAME", "TMPDIR", "LANG", "LC_ALL", "GH_TOKEN",
    ];
    variables
        .filter(|(key, _)| {
            key.to_str().is_some_and(|key| {
                KEPT.contains(&key)
                    || key == "GIT_CONFIG_COUNT"
                    || key.strip_prefix("GIT_CONFIG_KEY_").is_some_and(numbered)
                    || key.strip_prefix("GIT_CONFIG_VALUE_").is_some_and(numbered)
            })
        })
        .collect()
}

#[cfg(any(unix, test))]
fn numbered(suffix: &str) -> bool {
    !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(not(unix))]
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    Err("nessa-mcp shell currently requires Unix".into())
}

#[cfg(test)]
mod tests {
    use super::command_environment;

    #[test]
    fn commands_keep_the_github_sign_in_and_no_other_credential() {
        let given = [
            ("PATH", "/bin"),
            ("GH_TOKEN", "ghu_x"),
            ("GIT_CONFIG_COUNT", "2"),
            ("GIT_CONFIG_KEY_0", "credential.https://github.com.helper"),
            ("GIT_CONFIG_VALUE_1", "!f() { :; }; f"),
            ("GIT_CONFIG_GLOBAL", "/tmp/x"),
            ("GIT_CONFIG_KEY_", "x"),
            ("ANTHROPIC_API_KEY", "sk"),
            ("OPENAI_API_KEY", "sk"),
        ]
        .map(|(key, value)| (key.into(), value.into()));
        let kept: Vec<String> = command_environment(given.into_iter())
            .into_iter()
            .map(|(key, _)| key.into_string().unwrap())
            .collect();
        assert_eq!(
            kept,
            [
                "PATH",
                "GH_TOKEN",
                "GIT_CONFIG_COUNT",
                "GIT_CONFIG_KEY_0",
                "GIT_CONFIG_VALUE_1"
            ]
        );
    }
}
