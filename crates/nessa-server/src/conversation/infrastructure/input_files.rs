//! Local input placement capability; unsupported hosts refuse without I/O.
use crate::conversation::application::{
    EnvironmentFuture, LocalInputFile, LocalInputFilePlacement,
};
use nessa_sdk::application::agent_execution::providers::UserFileError;

#[cfg(unix)]
mod unix;

pub(crate) struct LocalInputFiles;
impl LocalInputFilePlacement for LocalInputFiles {
    fn place(
        &self,
        request: LocalInputFile,
    ) -> EnvironmentFuture<'static, Result<String, UserFileError>> {
        #[cfg(unix)]
        {
            unix::place(request)
        }
        #[cfg(not(unix))]
        {
            drop(request);
            Box::pin(async { Err(UserFileError::NotOffered) })
        }
    }
}
