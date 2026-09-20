//! Fatal reason → process exit code, read from
//! `protocol/defaults/gateway-exit-codes.json`.
//!
//! The exit code is how this process says why it stopped to something that
//! cannot see inside it. launchd records the exit code of the service it
//! supervises, and the desktop host reads that back rather than parsing this
//! process's log: a log line is prose written for a person, and prose is not a
//! contract. The table is one file, included by both sides, so a number cannot
//! mean one thing here and another there.
use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;

use nessa_auth::adapters::local::LocalStoreError;

use super::RunError;

const CODES_JSON: &str = include_str!("../../../../protocol/defaults/gateway-exit-codes.json");

#[derive(Debug, Deserialize)]
struct GatewayExitCodes {
    codes: BTreeMap<String, u8>,
}

static CODES: LazyLock<GatewayExitCodes> = LazyLock::new(|| {
    serde_json::from_str(CODES_JSON).expect("protocol/defaults/gateway-exit-codes.json must parse")
});

/// The name this error answers to in the shared table. Exhaustive by
/// construction: a new `RunError` variant does not compile until it is given
/// one, which is what keeps the table from falling behind the errors.
fn reason(error: &RunError) -> &'static str {
    match error {
        RunError::Environment(_) => "configuration",
        // "Invalid" is contents this build cannot make sense of — the schema
        // version among them — which is a different thing to tell someone
        // than a registry another process is holding or one that would not
        // open. Anything new in that family says the general thing, which
        // stays true, rather than blaming a version.
        RunError::Registry(LocalStoreError::Corrupt | LocalStoreError::Capacity) => {
            "credentialRegistryInvalid"
        }
        RunError::Registry(_) => "credentialRegistry",
        RunError::Authentication(_) => "authentication",
        RunError::Bind { source, .. } if source.kind() == std::io::ErrorKind::AddrInUse => {
            "portInUse"
        }
        RunError::Bind { .. } => "bind",
        RunError::Agent(_) => "agent",
        RunError::Serve(_) => "serve",
        RunError::Shutdown(_) => "shutdown",
    }
}

/// Code for `error`, or the unclassified failure when the table has no entry.
///
/// A missing entry is a table that has not caught up, not a reason to exit
/// successfully or to abort: the process still fails, and the host still
/// reports a service that will not start, without a cause it can name.
pub fn exit_code(error: &RunError) -> u8 {
    CODES
        .codes
        .get(reason(error))
        .copied()
        .filter(|code| *code != 0)
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    /// Every reason the table names is a distinct, non-zero code, and every
    /// error the server can fail with resolves to one of them.
    #[test]
    fn each_fatal_reason_has_its_own_non_zero_code() {
        let codes: Vec<u8> = CODES.codes.values().copied().collect();
        assert!(codes.iter().all(|code| *code != 0));
        let mut distinct = codes.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), codes.len());
        for error in [
            RunError::Environment(crate::env::EnvironmentError::Empty {
                variable: crate::env::HOST,
            }),
            RunError::Registry(LocalStoreError::Corrupt),
            RunError::Registry(LocalStoreError::Locked),
            RunError::Authentication("setup".into()),
            RunError::Agent("provider".into()),
            RunError::Bind {
                addr: "127.0.0.1:7420".into(),
                source: Error::from(ErrorKind::AddrInUse),
            },
            RunError::Bind {
                addr: "127.0.0.1:7420".into(),
                source: Error::from(ErrorKind::PermissionDenied),
            },
            RunError::Serve(Error::from(ErrorKind::BrokenPipe)),
            RunError::Shutdown(None),
        ] {
            assert!(CODES.codes.contains_key(reason(&error)), "{error}");
            assert_eq!(exit_code(&error), CODES.codes[reason(&error)]);
        }
    }

    /// The registry a build cannot read is the case the desktop reports as a
    /// registry problem, and a taken port is not the same answer as a bind
    /// that failed for another reason.
    #[test]
    fn the_reasons_the_desktop_names_are_told_apart() {
        let registry = exit_code(&RunError::Registry(LocalStoreError::Corrupt));
        let locked = exit_code(&RunError::Registry(LocalStoreError::Locked));
        let taken = exit_code(&RunError::Bind {
            addr: "127.0.0.1:7420".into(),
            source: Error::from(ErrorKind::AddrInUse),
        });
        let refused = exit_code(&RunError::Bind {
            addr: "127.0.0.1:7420".into(),
            source: Error::from(ErrorKind::PermissionDenied),
        });
        let authentication = exit_code(&RunError::Authentication("setup".into()));
        for pair in [
            (registry, taken),
            (registry, authentication),
            (registry, locked),
            (taken, refused),
        ] {
            assert_ne!(pair.0, pair.1);
        }
        // Nothing may collide with the unclassified failure.
        for code in [registry, locked, taken, refused, authentication] {
            assert_ne!(code, 1);
        }
    }
}
