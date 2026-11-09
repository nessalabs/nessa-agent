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
/// Provider credentials are not implicitly inherited by shell commands.
/// Nessa's GitHub sign-in stays in the gateway; a command gets neither its
/// token nor a git credential helper.
#[cfg(any(unix, test))]
fn command_environment(
    variables: impl Iterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    const KEPT: [&str; 7] = [
        "PATH", "HOME", "USER", "LOGNAME", "TMPDIR", "LANG", "LC_ALL",
    ];
    variables
        .filter(|(key, _)| key.to_str().is_some_and(|key| KEPT.contains(&key)))
        .collect()
}

#[cfg(not(unix))]
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    Err("nessa-mcp shell currently requires Unix".into())
}

#[cfg(test)]
mod tests {
    use super::command_environment;

    #[test]
    fn commands_drop_the_github_sign_in_and_other_credentials() {
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
        assert_eq!(kept, ["PATH"]);
    }
}
