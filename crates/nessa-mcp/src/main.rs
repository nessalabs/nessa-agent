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
    // Credentials used by the provider are not implicitly inherited by shell commands.
    let environment = [
        "PATH", "HOME", "USER", "LOGNAME", "TMPDIR", "LANG", "LC_ALL",
    ]
    .into_iter()
    .filter_map(|key| std::env::var_os(key).map(|value| (key.into(), value)))
    .collect();
    let audit = Arc::new(PrivateAudit::new(directory)?);
    let service = Arc::new(ShellService::new(
        Arc::new(ShepherdRunner::new(supervisor.clone(), environment)),
        audit,
    ));
    let result = mcp::serve(service, workspace, uuid::Uuid::new_v4().to_string()).await;
    let cleanup = supervisor.shutdown().await?;
    if !cleanup.scopes.iter().all(|scope| scope.all_verified()) {
        return Err("Shepherd could not verify process cleanup".into());
    }
    result
}

#[cfg(not(unix))]
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    Err("nessa-mcp shell currently requires Unix".into())
}
