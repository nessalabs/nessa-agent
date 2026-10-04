//! The executable owns the private cache, wall clock, revision and output handles.
//! `profile` acquires the bounded private profile: the device's private state
//! directory and the gateway's native address. `device` pairs and reads the
//! pinned enrollment status. `online` admits each run by that status, then
//! consumes the protected session, its facades and finite drivers before JSON
//! presentation.
mod device;
mod online;
mod profile;
use crate::composition::local_auth::SystemClock;
use crate::product::generated::{
    MAX_PHYSICAL_RECORD_PAYLOAD_BYTES, MAX_RECORD_PAGE_PAYLOAD_BYTES, MAX_RECORD_PAGE_RECORDS,
};
use crate::read_only_sync::application::CachePolicy;
use crate::read_only_sync::entrypoint::{parse, run_local, Command, CommandError};
use crate::read_only_sync::infrastructure::cache::ReadOnlyCache;
use nessa_sync::replication::domain::Limits;
use std::io::{Read, Write};
use std::process::ExitCode;
use std::sync::Arc;
use uuid::Uuid;

/// Runs explicit online and retained-data commands from the standalone example's argument contract.
pub fn run_read_only_example(arguments: Vec<String>) -> ExitCode {
    let result = execute(
        &arguments,
        &mut std::io::stdin().lock(),
        &mut std::io::stdout().lock(),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(std::io::stderr().lock(), "{error}");
            ExitCode::FAILURE
        }
    }
}

pub(super) fn execute(
    arguments: &[String],
    input: &mut dyn Read,
    output: &mut dyn Write,
) -> Result<(), CommandError> {
    let command = parse(arguments)?;
    let limits = Limits::new(
        MAX_RECORD_PAGE_RECORDS,
        MAX_RECORD_PAGE_PAYLOAD_BYTES,
        MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
    )
    .map_err(|_| CommandError::Arguments)?;
    let policy = CachePolicy::new(256 * 1024 * 1024, 16 * 1024 * 1024, limits)
        .map_err(CommandError::Cache)?;
    if matches!(command, Command::Pair { .. } | Command::Status { .. }) {
        return online::execute_device(&command, policy, input, output);
    }
    let Command::Local(local) = &command else {
        return online::execute(&command, policy, output);
    };
    let mut cache = ReadOnlyCache::open(&local.cache, policy, Arc::new(SystemClock))
        .map_err(CommandError::Cache)?;
    run_local(&command, &mut cache, Uuid::new_v4(), output)
}
