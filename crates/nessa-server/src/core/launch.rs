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
        Self::resolve(
            command,
            Environment::service_generation_from_system(),
            Environment::log_directory_from_system().ok().flatten(),
        )
    }

    /// The rule itself, apart from the reads. Both or neither: a managed launch
    /// with no generation cannot correlate a record to anything, and one with
    /// no log directory has nowhere to put it. Either way the honest answer is
    /// standalone, and this ending has to speak for itself in an exit code.
    fn resolve(command: &Command, generation: Option<String>, logs: Option<PathBuf>) -> Self {
        match (command, generation, logs) {
            (Command::Desktop(_), Some(generation), Some(logs)) => {
                Self::Managed(Managed { generation, logs })
            }
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

    /// A launch registered under `generation` and writing to `logs`, for a test
    /// describing one.
    ///
    /// There is no such constructor in a running process. [`Launch::resolve`]
    /// is the only route, so the fact the design says is established once at
    /// the edge cannot be minted anywhere else.
    #[cfg(test)]
    pub(crate) fn new(generation: String, logs: PathBuf) -> Self {
        Self { generation, logs }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::entrypoint::{parse, LocalProvisioning};

    const GENERATION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const RUNTIME: &str = "/Users/someone/Library/Application Support/Nessa/gateway-runtimes/x";

    /// The arguments `src-tauri/src/gateway/infrastructure/macos/staging.rs`
    /// puts in the plist, after the program itself. Written out rather than
    /// referenced because the two crates do not share a dependency; its own
    /// test there asserts the plist still holds these.
    fn registered_arguments() -> Vec<String> {
        ["server", "--desktop-runtime", RUNTIME]
            .into_iter()
            .map(String::from)
            .collect()
    }

    /// The whole feature turns on this mapping: what launchd is configured to
    /// run has to arrive here as `Command::Desktop`, and with the generation
    /// the plist sets it has to be a managed launch.
    #[test]
    fn what_launchd_is_configured_to_run_resolves_to_a_managed_launch() {
        let command = parse(&registered_arguments()).expect("the plist's own arguments");
        assert_eq!(command, Command::Desktop(RUNTIME.into()));

        let logs = PathBuf::from("/Users/someone/.nessa/logs");
        let launch = Launch::resolve(&command, Some(GENERATION.into()), Some(logs.clone()));
        assert_eq!(
            launch,
            Launch::Managed(Managed::new(GENERATION.into(), logs.clone()))
        );
        let managed = launch.managed().expect("managed");
        assert_eq!(managed.generation(), GENERATION);
        assert_eq!(managed.logs(), logs);
    }

    /// Both or neither. Without a generation there is nothing to correlate a
    /// record to, and without a log directory there is nowhere to put one, so
    /// either missing leaves the ending to speak for itself in an exit code.
    #[test]
    fn a_registration_this_process_cannot_name_is_not_one_it_answers_for() {
        let command = Command::Desktop(RUNTIME.into());
        let logs = PathBuf::from("/Users/someone/.nessa/logs");
        for (generation, logs) in [
            (None, Some(logs.clone())),
            (Some(GENERATION.to_owned()), None),
            (None, None),
        ] {
            assert_eq!(
                Launch::resolve(&command, generation.clone(), logs.clone()),
                Launch::Standalone,
                "{generation:?} {logs:?}"
            );
        }
    }

    /// Only the command launchd is configured to run can be a managed launch.
    /// A `nessa server` someone typed shares the stage's data directory, and a
    /// generation left in its environment must not make it the desktop's
    /// service.
    #[test]
    fn a_command_nobody_registered_is_never_a_managed_launch() {
        let logs = PathBuf::from("/Users/someone/.nessa/logs");
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
                Launch::resolve(&command, Some(GENERATION.into()), Some(logs.clone())),
                Launch::Standalone,
                "{command:?}"
            );
            assert_eq!(
                Launch::from_system(&command),
                Launch::Standalone,
                "{command:?}"
            );
        }
    }

    /// A standalone launch answers for no registration, so there is nothing
    /// for it to publish under or take away.
    #[test]
    fn a_standalone_launch_has_no_registration_to_act_for() {
        assert_eq!(Launch::Standalone.managed(), None);
    }
}
