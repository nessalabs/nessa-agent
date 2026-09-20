//! Who started this process, and therefore what its ending is allowed to mean.
//!
//! Three things in this binary behave differently for the desktop's background
//! service than for a `nessa` command someone typed: whether a failure may exit
//! zero to stop launchd relaunching it, whether the recovery record beside the
//! log may be written, and whether it may be removed. All three are the same
//! question, so they are answered once, here, from one fact.
//!
//! That fact is not sniffed out later from an environment variable at the point
//! of use. It is resolved at the process edge, from the command launchd is
//! configured to run (`server --desktop-runtime <tree>`) together with the
//! service generation only the desktop host's plist sets, and handed down. A
//! process that cannot establish both is standalone, which is the answer that
//! changes nothing: its failures keep the exit codes the shared table gives
//! them, and it leaves another registration's recovery evidence alone.
//!
//! ```text
//! launchd ──► server --desktop-runtime + NESSA_SERVICE_GENERATION
//!                        │
//!                  Launch::Managed(generation, logs)
//!                        ├─ may bound its own log
//!                        ├─ may publish a record for its generation
//!                        └─ may forget one its generation wrote
//!
//! anything else ──► Launch::Standalone ── touches none of that
//! ```
use std::path::{Path, PathBuf};

use crate::cli::entrypoint::Command;
use crate::env::Environment;

/// What started this process.
#[derive(Debug, PartialEq, Eq)]
pub enum Launch {
    /// The desktop's background service, under the generation its host
    /// registered it with.
    Managed(Managed),
    /// Everything else: a developer's server, a CLI subcommand, a test.
    Standalone,
}

/// A launch the desktop host owns, and the two things that gives it: the
/// registration it answers for, and the directory launchd redirects it to.
#[derive(Debug, PartialEq, Eq)]
pub struct Managed {
    generation: String,
    logs: PathBuf,
}

impl Launch {
    /// Resolve from the command this process was given and the environment its
    /// plist fixed. Read once, at the edge; nothing below re-derives it.
    pub fn from_system(command: &Command) -> Self {
        let Command::Desktop(_) = command else {
            return Self::Standalone;
        };
        // Both or neither. A managed launch with no generation cannot correlate
        // a record to anything, and one with no log directory has nowhere to
        // put it; either way the honest answer is that this ending has to speak
        // for itself in an exit code.
        match (
            Environment::service_generation_from_system(),
            Environment::log_directory_from_system(),
        ) {
            (Some(generation), Ok(Some(logs))) => Self::Managed(Managed { generation, logs }),
            _ => Self::Standalone,
        }
    }

    /// Forget the record this launch supersedes, if it has one to forget.
    ///
    /// The composition root calls this on its way into serving. A standalone
    /// run has nothing to forget — the record in that directory belongs to a
    /// registration it is not — and a managed one removes only what its own
    /// generation wrote.
    pub fn forget_startup_failure(&self) {
        if let Self::Managed(managed) = self {
            super::startup_failure::forget(managed);
        }
    }

    /// The managed launch, when this is one.
    pub(super) fn managed(&self) -> Option<&Managed> {
        match self {
            Self::Managed(managed) => Some(managed),
            Self::Standalone => None,
        }
    }
}

impl Managed {
    /// The identity a record has to name to be about this registration.
    pub(super) fn generation(&self) -> &str {
        &self.generation
    }
    /// Where launchd sends this service's output, and where its record lives.
    pub(super) fn logs(&self) -> &Path {
        &self.logs
    }

    /// The launch the desktop host registered under `generation`, writing to
    /// `logs`. The composition edge builds one from the process it is in; a
    /// test builds one from the registration it is describing.
    pub fn new(generation: String, logs: PathBuf) -> Self {
        Self { generation, logs }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::entrypoint::LocalProvisioning;

    /// Only the command launchd is configured to run can be a managed launch.
    /// A `nessa server` someone typed shares the stage's data directory and
    /// must not be mistaken for the desktop's service because of it.
    #[test]
    fn a_command_nobody_registered_is_never_a_managed_launch() {
        for command in [
            Command::Server(LocalProvisioning::Manual),
            Command::Server(LocalProvisioning::Automatic),
            Command::Help,
            Command::Offline(vec!["auth".into(), "init".into()]),
            Command::Doctor {
                credential_file: None,
            },
        ] {
            assert_eq!(
                Launch::from_system(&command),
                Launch::Standalone,
                "{command:?}"
            );
        }
    }

    /// A managed launch answers for exactly one registration, and a record has
    /// to name that one.
    #[test]
    fn a_managed_launch_carries_the_registration_it_answers_for() {
        let managed = Managed::new("a".repeat(64), PathBuf::from("/data/logs"));
        assert_eq!(managed.generation(), "a".repeat(64));
        assert_eq!(managed.logs(), Path::new("/data/logs"));
        assert_eq!(
            Launch::Managed(Managed::new("a".repeat(64), PathBuf::from("/data/logs"))).managed(),
            Some(&managed)
        );
        assert_eq!(Launch::Standalone.managed(), None);
    }
}
