//! The executable owns the private cache, wall clock, revision and output handles.
//! `profile` acquires bounded private inputs and delegates current endpoint
//! discovery to the canonical publication owner. `online` consumes authenticated
//! scope discovery, socket facades and finite drivers before JSON presentation.
mod online;
mod profile;
use crate::composition::local_auth::SystemClock;
use crate::product::generated::{
    MAX_PHYSICAL_RECORD_PAYLOAD_BYTES, MAX_RECORD_PAGE_PAYLOAD_BYTES, MAX_RECORD_PAGE_RECORDS,
};
use crate::read_only_sync::{
    application::CachePolicy,
    entrypoint::{parse, run_local, Command, CommandError},
    infrastructure::cache::ReadOnlyCache,
};
use nessa_sync::replication::domain::Limits;
use std::{io::Write, process::ExitCode, sync::Arc};

/// Runs explicit online and retained-data commands from the standalone example's argument contract.
pub fn run_read_only_example(arguments: Vec<String>) -> ExitCode {
    let result = execute(&arguments, &mut std::io::stdout().lock());
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(std::io::stderr().lock(), "{error}");
            ExitCode::FAILURE
        }
    }
}

pub(super) fn execute(arguments: &[String], output: &mut dyn Write) -> Result<(), CommandError> {
    let command = parse(arguments)?;
    let limits = Limits::new(
        MAX_RECORD_PAGE_RECORDS,
        MAX_RECORD_PAGE_PAYLOAD_BYTES,
        MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
    )
    .map_err(|_| CommandError::Arguments)?;
    let policy = CachePolicy::new(256 * 1024 * 1024, 16 * 1024 * 1024, limits)
        .map_err(CommandError::Cache)?;
    let Command::Local(local) = &command else {
        return online::execute(&command, policy, output);
    };
    let mut cache = ReadOnlyCache::open(&local.cache, policy, Arc::new(SystemClock))
        .map_err(CommandError::Cache)?;
    run_local(&command, &mut cache, uuid::Uuid::new_v4(), output)
}
