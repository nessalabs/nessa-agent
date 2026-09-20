//! Substitutes for the gateway's ports, kept next to the ports they stand in
//! for so every module that bootstraps a [`Gateway`](super::Gateway) in a test
//! uses the same ones.
use super::{LoginShellError, LoginShellPath};
use crate::gateway::domain::value_objects::SearchPath;
use std::sync::Arc;

/// A login shell with a fixed answer — the path it reports, or the reason it
/// reported none.
pub(crate) struct FixedLoginShell(pub(crate) Result<SearchPath, LoginShellError>);
impl LoginShellPath for FixedLoginShell {
    fn resolve(&self) -> Result<SearchPath, LoginShellError> {
        self.0.clone()
    }
}

/// A login shell that answers with the system path: enough for a test whose
/// subject is something else.
pub(crate) fn system_login_shell() -> Arc<dyn LoginShellPath> {
    Arc::new(FixedLoginShell(Ok(SearchPath::system())))
}
