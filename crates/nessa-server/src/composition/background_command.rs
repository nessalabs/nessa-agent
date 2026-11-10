//! VM command helper composition. Account environment is cleared by Shepherd;
//! no agent login or gateway credential is passed into the command.
use crate::{
    conversation::{
        application::background_tasks::BackgroundRunner,
        domain::BackgroundCommand,
        infrastructure::{
            background_tasks::{io::BackgroundIo, transport},
            ShepherdBackgroundTasks,
        },
    },
    core::RunError,
};
use std::{os::fd::AsFd, sync::Arc};

pub(super) async fn execute(cwd: String, command: BackgroundCommand) -> Result<(), RunError> {
    let environment = super::mcp_servers::server_environment(|key| std::env::var_os(key))
        .into_iter()
        .collect();
    let input = BackgroundIo::from_fd(std::io::stdin().as_fd().try_clone_to_owned()?)?;
    let output = BackgroundIo::from_fd(std::io::stdout().as_fd().try_clone_to_owned()?)?;
    let runner = ShepherdBackgroundTasks::new(
        shepherd::SupervisorBuilder::new().build(),
        environment,
        Arc::new(shepherd::SystemClock),
    );
    let served = transport::serve(&runner, command, cwd, input, output).await;
    let cleanup = runner.shutdown().await;
    served?;
    cleanup.map_err(|error| RunError::Agent(format!("background cleanup: {}", error.code())))
}
