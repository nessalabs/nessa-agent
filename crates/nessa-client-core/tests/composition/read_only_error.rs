//! Public command diagnostics preserve the existing stderr evidence exactly.
use super::{CommandError, CommandFailure};
use crate::{composition, read_only_sync::application::CacheError};
use nessa_sdk::application::agent_execution::sessions::StorageError;
#[test]
fn command_failure_preserves_diagnostics() {
    let mut output = vec![];
    let error = composition::execute(&[], &mut std::io::empty(), &mut output).unwrap_err();
    assert!(output.is_empty());
    assert_eq!(format!("{error:?}"), "Arguments");
    assert_eq!(error.to_string(), CommandFailure::Arguments.to_string());
    let error = CommandError::new(CommandFailure::Cache(CacheError::Transcript(
        StorageError::Io("owned diagnostic".into()),
    )));
    assert_eq!(
        format!("{error:?}"),
        "Cache(Transcript(Io(\"owned diagnostic\")))"
    );
    assert_eq!(error.to_string(), "SDK transcript rejected saved data");
}
