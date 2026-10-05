//! The executable owns the private cache, wall clock, revision and output handles.
//! `clock` supplies independent wall and monotonic adapters. `profile` acquires
//! the bounded private profile: the device's private state directory and gateway
//! address. `device` pairs and reads the pinned enrollment status. `online` admits each run by that status, then
//! consumes the protected session, its facades and finite drivers before JSON
//! presentation.
mod clock;
mod device;
pub(crate) mod error;
mod online;
mod profile;
use crate::composition::clock::SystemClock;
use crate::read_only_sync::application::CachePolicy;
use crate::read_only_sync::entrypoint::{
    parse, run_local, Command, CommandError as CommandFailure,
};
use crate::read_only_sync::infrastructure::cache::ReadOnlyCache;
use crate::CommandError;
use nessa_protocol::product::generated::{
    MAX_PHYSICAL_RECORD_PAYLOAD_BYTES, MAX_RECORD_PAGE_PAYLOAD_BYTES, MAX_RECORD_PAGE_RECORDS,
};
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

/// Executes the current example command with caller-owned input and JSON output.
/// Online commands acquire the profile's private state and cache; local commands
/// open only the selected cache. The immutable error preserves diagnostic text;
/// JSON output owns machine-readable outcomes. Callers must not parse Display.
pub fn execute(
    arguments: &[String],
    input: &mut dyn Read,
    output: &mut dyn Write,
) -> Result<(), CommandError> {
    execute_command(arguments, input, output).map_err(CommandError::new)
}

fn execute_command(
    arguments: &[String],
    input: &mut dyn Read,
    output: &mut dyn Write,
) -> Result<(), CommandFailure> {
    let command = parse(arguments)?;
    let limits = Limits::new(
        MAX_RECORD_PAGE_RECORDS,
        MAX_RECORD_PAGE_PAYLOAD_BYTES,
        MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
    )
    .map_err(|_| CommandFailure::Arguments)?;
    let policy = CachePolicy::new(256 * 1024 * 1024, 16 * 1024 * 1024, limits)
        .map_err(CommandFailure::Cache)?;
    if matches!(command, Command::Pair { .. } | Command::Status { .. }) {
        return online::execute_device(&command, policy, input, output);
    }
    let Command::Local(local) = &command else {
        return online::execute(&command, policy, output);
    };
    let mut cache = ReadOnlyCache::open(&local.cache, policy, Arc::new(SystemClock))
        .map_err(CommandFailure::Cache)?;
    run_local(&command, &mut cache, Uuid::new_v4(), output)
}
