//! Immutable public diagnostic result over the unchanged internal command cause.
use crate::read_only_sync::entrypoint::CommandError as CommandFailure;
use std::fmt::{Debug, Display, Formatter, Result};

/// Diagnostic command failure retaining the original typed cause privately.
/// Debug and Display preserve existing diagnostics; JSON command output owns
/// machine-readable outcomes. The error retains values, never command resources.
///
/// The retained cause cannot be replaced by a caller:
/// ```compile_fail
/// use nessa_client_core::CommandError;
/// fn replace(error: &mut CommandError, other: CommandError) {
///     error.0 = other.0;
/// }
/// ```
pub struct CommandError(CommandFailure);
impl CommandError {
    pub(crate) fn new(cause: CommandFailure) -> Self {
        Self(cause)
    }
}
impl Debug for CommandError {
    fn fmt(&self, output: &mut Formatter<'_>) -> Result {
        Debug::fmt(&self.0, output)
    }
}
impl Display for CommandError {
    fn fmt(&self, output: &mut Formatter<'_>) -> Result {
        Display::fmt(&self.0, output)
    }
}

#[cfg(test)]
#[path = "../../tests/composition/read_only_error.rs"]
mod tests;
