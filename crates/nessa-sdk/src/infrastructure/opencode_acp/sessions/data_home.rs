//! Resolves the account-data directory used by an OpenCode child process.

use crate::application::agent_execution::agents::AgentError;
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    path::PathBuf,
};

/// Resolve the data directory OpenCode uses for account state.
///
/// `environment` is applied first and `credential_environment` second, matching
/// the child-process launch order. An empty `XDG_DATA_HOME` has its standard
/// meaning and falls back to a nonempty `HOME/.local/share`; an empty later-map
/// value therefore overrides an earlier-map value before fallback is applied.
///
/// The returned directory is the exact value the OpenCode binding preserves
/// while replacing the child's other home and XDG directories. Callers may use
/// it to inspect the same `opencode/auth.json` the child will consume, without
/// reading or returning credential contents.
///
/// # Errors
/// Returns [`AgentError::Configuration`] when neither map supplies a nonempty
/// effective `XDG_DATA_HOME` or `HOME`.
///
/// ```
/// use nessa_sdk::infrastructure::opencode_acp::sessions::effective_data_home;
/// use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};
///
/// let environment = BTreeMap::from([(
///     OsString::from("HOME"),
///     OsString::from("/home/person"),
/// )]);
/// assert_eq!(
///     effective_data_home(&environment, &BTreeMap::new())?,
///     PathBuf::from("/home/person/.local/share"),
/// );
/// # Ok::<(), nessa_sdk::application::agent_execution::agents::AgentError>(())
/// ```
pub fn effective_data_home(
    environment: &BTreeMap<OsString, OsString>,
    credential_environment: &BTreeMap<OsString, OsString>,
) -> Result<PathBuf, AgentError> {
    let effective = |key: &str| {
        credential_environment
            .get(OsStr::new(key))
            .or_else(|| environment.get(OsStr::new(key)))
    };
    if let Some(data_home) = effective("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(data_home));
    }
    effective("HOME")
        .filter(|value| !value.is_empty())
        .map(|home| PathBuf::from(home).join(".local").join("share"))
        .ok_or_else(|| {
            AgentError::Configuration(
                "Opencode requires HOME or XDG_DATA_HOME so its account data remains addressable while configuration is isolated".into(),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonempty_xdg_data_home_precedes_home() {
        let environment = BTreeMap::from([
            (OsString::from("HOME"), OsString::from("/home/person")),
            (
                OsString::from("XDG_DATA_HOME"),
                OsString::from("/context/data"),
            ),
        ]);

        assert_eq!(
            effective_data_home(&environment, &BTreeMap::new()).unwrap(),
            PathBuf::from("/context/data")
        );
    }

    #[test]
    fn later_empty_xdg_value_overrides_then_falls_back_to_effective_home() {
        let environment = BTreeMap::from([
            (OsString::from("HOME"), OsString::from("/context/home")),
            (
                OsString::from("XDG_DATA_HOME"),
                OsString::from("/context/data"),
            ),
        ]);
        let credentials = BTreeMap::from([
            (OsString::from("HOME"), OsString::from("/credential/home")),
            (OsString::from("XDG_DATA_HOME"), OsString::new()),
        ]);

        assert_eq!(
            effective_data_home(&environment, &credentials).unwrap(),
            PathBuf::from("/credential/home/.local/share")
        );
    }

    #[test]
    fn empty_effective_roots_are_rejected() {
        let environment = BTreeMap::from([(
            OsString::from("XDG_DATA_HOME"),
            OsString::from("/context/data"),
        )]);
        let credentials = BTreeMap::from([(OsString::from("XDG_DATA_HOME"), OsString::new())]);

        assert!(matches!(
            effective_data_home(&environment, &credentials),
            Err(AgentError::Configuration(_))
        ));
    }
}
