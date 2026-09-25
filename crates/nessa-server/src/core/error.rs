//! Fatal process errors — config, bind, and serve failures.
//!
//! Returned from [`super::bootstrap::run`] and [`crate::composition::CompositionRoot::serve`].
//! [`super::ending`] turns one into how the process ends: a logged sentence, an
//! exit status launchd reads, and — when starting again would not help — a
//! record the desktop host reads instead of that status.
//!
//! Re-exported at `crate::core::RunError`.

use crate::browser_session::adapters::JournalOpenError;
use crate::conversation::application::ConversationError;
use crate::env::EnvironmentError;
use nessa_auth::adapters::local::LocalStoreError;
use nessa_auth::application::credential_registry::CredentialRegistryAuditError;
#[cfg(unix)]
use nessa_local_database::OpenError;
use std::fmt;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

/// Fatal errors that stop the server process.
#[derive(Debug)]
pub enum RunError {
    Environment(EnvironmentError),
    /// The command line itself did not name something this binary can run.
    /// Every command is parsed before it is dispatched, so this is the one
    /// failure any invocation can end in, whatever it was trying to do.
    Usage(String),
    /// The credential registry on disk could not be opened. Kept typed rather
    /// than flattened into `Authentication`, because it is the one setup
    /// failure the desktop host reports in its own words, and it learns which
    /// failure this was from the exit code this variant chooses.
    Registry(RegistryFailure),
    /// A store the gateway cannot serve without holds something this build
    /// cannot read: another version, or a file that is not a database.
    /// Starting again reads the same file
    /// (docs/adr/todo/202-versioned-local-datasets.md).
    Dataset(DatasetRefusal),
    /// Product authentication failed to initialize; contains no credential material.
    Authentication(String),
    /// Invalid or unavailable configured agent provider.
    Agent(String),
    /// The prepared runtime this process was handed is missing, unreadable, or
    /// not the one its registration was fingerprinted against. Typed apart from
    /// `Agent` because nothing about starting again changes any of that, and
    /// the host has its own sentence for it.
    Runtime(String),
    Bind {
        addr: String,
        source: io::Error,
    },
    Serve(io::Error),
    /// Conversations did not confirm cleanup and audit delivery on the way down.
    /// The HTTP server itself finished; this is what shutdown could not prove.
    /// `None` means shutdown never reported at all — unknown, which is its own
    /// fact and not the same as a reported failure.
    Shutdown(Option<ConversationError>),
}

impl RunError {
    /// A gateway-scope store that did not open. What the file holds is a
    /// [`RunError::Dataset`]; anything that can clear — a directory, I/O, a
    /// lock — stays `Agent`, which is retried. Conversations are composed
    /// only on Unix, so this is too.
    #[cfg(unix)]
    pub(crate) fn opening(dataset: Dataset, path: &Path, cause: OpenError) -> Self {
        match cause {
            OpenError::Version { .. } | OpenError::Unreadable(_) => {
                Self::Dataset(DatasetRefusal::new(dataset, path, cause))
            }
            cause => Self::Agent(format!("{dataset} at {}: {cause}", path.display())),
        }
    }

    /// The browser-session journal that did not open. A journal replay
    /// refuses is a [`RunError::Dataset`]; one that could not be opened,
    /// locked or synced stays `Authentication`, which is retried.
    pub(crate) fn opening_browser_sessions(path: &Path, cause: JournalOpenError) -> Self {
        match cause {
            JournalOpenError::Unreadable { .. } => {
                Self::Dataset(DatasetRefusal::new(Dataset::BrowserSessions, path, cause))
            }
            JournalOpenError::Unavailable => Self::Authentication(cause.to_string()),
        }
    }

    pub(crate) fn registry(
        primary: LocalStoreError,
        audit: Option<CredentialRegistryAuditError>,
    ) -> Self {
        Self::Registry(RegistryFailure::new(primary, audit))
    }
}

/// The stores whose refusal refuses the gateway, named for the sentence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dataset {
    ConversationMetadata,
    BrowserSessions,
}

impl fmt::Display for Dataset {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ConversationMetadata => "conversation metadata",
            Self::BrowserSessions => "browser sessions",
        })
    }
}

/// Which store refused, where it is, and what the opener found.
#[derive(Debug)]
pub struct DatasetRefusal {
    dataset: Dataset,
    path: PathBuf,
    /// The opener's own error, for the sentence and the log; nothing
    /// branches on it.
    cause: Box<dyn std::error::Error + Send + Sync>,
}

impl DatasetRefusal {
    fn new(
        dataset: Dataset,
        path: &Path,
        cause: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            dataset,
            path: path.to_path_buf(),
            cause: Box::new(cause),
        }
    }

    pub fn dataset(&self) -> Dataset {
        self.dataset
    }
}

impl fmt::Display for DatasetRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} at {} cannot be read by this build: {}",
            self.dataset,
            self.path.display(),
            self.cause
        )
    }
}

/// A failed registry open and the independent result of auditing its refusal.
///
/// The store failure remains primary. Audit delivery can fail beside it but
/// never replaces what was refused or changes ending policy.
#[derive(Debug)]
pub struct RegistryFailure {
    primary: LocalStoreError,
    audit: Option<CredentialRegistryAuditError>,
}

impl RegistryFailure {
    pub(crate) fn new(
        primary: LocalStoreError,
        audit: Option<CredentialRegistryAuditError>,
    ) -> Self {
        Self { primary, audit }
    }

    pub(crate) fn primary(&self) -> &LocalStoreError {
        &self.primary
    }
}

impl From<LocalStoreError> for RegistryFailure {
    fn from(primary: LocalStoreError) -> Self {
        Self::new(primary, None)
    }
}

impl fmt::Display for RegistryFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.primary)?;
        if let Some(audit) = &self.audit {
            write!(
                formatter,
                "; credential registry refusal audit was not recorded: {audit}"
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for RegistryFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.primary)
    }
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Agent(message) => write!(f, "agent setup failed: {message}"),
            Self::Runtime(message) => write!(f, "prepared runtime unusable: {message}"),
            // Said as it is. A usage message already names what was wrong with
            // the command, and the help text is one of the things it can be, so
            // there is no failing subsystem to announce in front of it.
            Self::Usage(message) => write!(f, "{message}"),
            // Same sentence a flattened registry error used to produce: this
            // is still what authentication setup failed on.
            Self::Registry(error) => write!(f, "authentication setup failed: {error}"),
            Self::Dataset(refusal) => write!(f, "stored data refused: {refusal}"),
            Self::Authentication(message) => write!(f, "authentication setup failed: {message}"),
            Self::Environment(error) => write!(f, "invalid configuration: {error}"),
            Self::Bind { addr, source } => match source.kind() {
                ErrorKind::AddrInUse => write!(
                    f,
                    "port already in use at {addr}; stop the other process or set NESSA_PORT"
                ),
                _ => write!(f, "failed to bind {addr}: {source}"),
            },
            Self::Serve(source) => write!(f, "server stopped: {source}"),
            Self::Shutdown(Some(error)) => {
                write!(f, "shutdown did not confirm all cleanup: {error}")
            }
            Self::Shutdown(None) => {
                write!(f, "shutdown never reported whether cleanup completed")
            }
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Environment(error) => Some(error),
            Self::Registry(error) => Some(error),
            Self::Dataset(refusal) => Some(&*refusal.cause),
            Self::Usage(_) | Self::Authentication(_) | Self::Agent(_) | Self::Runtime(_) => None,
            Self::Bind { source, .. } => Some(source),
            Self::Serve(source) => Some(source),
            Self::Shutdown(error) => error.as_ref().map(|error| error as _),
        }
    }
}

impl From<EnvironmentError> for RunError {
    fn from(value: EnvironmentError) -> Self {
        Self::Environment(value)
    }
}

impl From<io::Error> for RunError {
    fn from(value: io::Error) -> Self {
        Self::Serve(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use crate::conversation::infrastructure::LocalConversationStore;
    use crate::env::{EnvironmentError, HOST};

    #[test]
    fn an_unconfirmed_shutdown_says_which_kind_it_was() {
        let reported = RunError::Shutdown(Some(ConversationError::Audit));
        assert!(reported.to_string().contains("did not confirm all cleanup"));
        // The typed failure is the source, so a caller can match on it.
        assert!(std::error::Error::source(&reported).is_some());

        let silent = RunError::Shutdown(None);
        assert!(silent.to_string().contains("never reported"));
        // Nothing was reported, so there is nothing to be the source.
        assert!(std::error::Error::source(&silent).is_none());
    }

    #[cfg(unix)]
    /// Opens `metadata.sqlite3` in a private directory, after `prepare` has
    /// left something there, and says what composition would end with.
    fn opening_metadata(prepare: impl FnOnce(&Path)) -> RunError {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("conversations");
        nessa_local_storage::create_directory(&root).unwrap();
        let path = root.join("metadata.sqlite3");
        prepare(&path);
        let before = std::fs::read(&path).ok();
        let Err(cause) = LocalConversationStore::open(&path) else {
            panic!("the store opened");
        };
        // Refused, and never rewritten: the file is whatever it was.
        assert_eq!(std::fs::read(&path).ok(), before);
        RunError::opening(Dataset::ConversationMetadata, &path, cause)
    }

    #[cfg(unix)]
    fn assert_refused_for_good(error: &RunError) {
        assert!(
            matches!(error, RunError::Dataset(refusal)
                if refusal.dataset() == Dataset::ConversationMetadata),
            "{error}"
        );
        assert_eq!(super::super::exit_code::reason(error), "datasetRefused");
        assert_eq!(
            super::super::restart::restart(error),
            super::super::restart::Restart::Pointless
        );
        assert!(error.to_string().contains("conversation metadata"));
    }

    #[cfg(unix)]
    /// Row G3 of ADR 202: a newer build's file, and a file with tables and
    /// no version, are both another version.
    #[test]
    fn a_metadata_database_at_another_version_refuses_the_gateway_for_good() {
        for definition in [
            "CREATE TABLE later (id TEXT) STRICT;\nPRAGMA user_version = 99;\n",
            "CREATE TABLE unversioned (id TEXT) STRICT;\n",
        ] {
            let error = opening_metadata(|path| {
                // Private first, as the opener would make it, so that it is
                // the version and not the privacy check that refuses it.
                drop(
                    nessa_local_storage::open(path, nessa_local_storage::OpenMode::CreateNew)
                        .unwrap(),
                );
                let connection = nessa_local_database::rusqlite::Connection::open(path).unwrap();
                connection.execute_batch(definition).unwrap();
            });
            assert_refused_for_good(&error);
        }
    }

    #[cfg(unix)]
    /// Row G4 of ADR 202.
    #[test]
    fn a_file_that_is_not_a_database_refuses_the_gateway_for_good() {
        let error = opening_metadata(|path| {
            let mut file =
                nessa_local_storage::open(path, nessa_local_storage::OpenMode::CreateNew).unwrap();
            std::io::Write::write_all(&mut file, &[7; 4096]).unwrap();
        });
        assert_refused_for_good(&error);
    }

    #[cfg(unix)]
    /// Row G5 of ADR 202: a directory that is missing, or not private, can
    /// be put right, so it stays a failure launchd retries.
    #[test]
    fn a_metadata_directory_that_cannot_be_used_is_still_retried() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("absent").join("metadata.sqlite3");
        let Err(cause) = LocalConversationStore::open(&path) else {
            panic!("the store opened");
        };
        let error = RunError::opening(Dataset::ConversationMetadata, &path, cause);
        assert!(matches!(error, RunError::Agent(_)), "{error}");
        assert_eq!(
            super::super::restart::restart(&error),
            super::super::restart::Restart::Worthwhile
        );
    }

    /// Rows G3/G4 of ADR 202 for the browser-session journal: what replay
    /// refuses is the same file next time.
    #[test]
    fn a_browser_session_journal_replay_refuses_stops_the_gateway_for_good() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("browser-sessions.jsonl");
        // Private, as the journal would make it, so that replay and not the
        // privacy check refuses it.
        let mut file =
            nessa_local_storage::open(&path, nessa_local_storage::OpenMode::CreateNew).unwrap();
        std::io::Write::write_all(&mut file, b"not a record\n").unwrap();
        drop(file);
        let Err(cause) = crate::browser_session::adapters::PersistentSessions::open(&path, 100)
        else {
            panic!("the journal opened");
        };
        let error = RunError::opening_browser_sessions(&path, cause);
        assert!(
            matches!(&error, RunError::Dataset(refusal)
                if refusal.dataset() == Dataset::BrowserSessions),
            "{error}"
        );
        assert_eq!(super::super::exit_code::reason(&error), "datasetRefused");
        assert_eq!(
            super::super::restart::restart(&error),
            super::super::restart::Restart::Pointless
        );
        assert!(error.to_string().contains("line 1"), "{error}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not a record\n");
    }

    /// Row G5 for the journal: held by another opener clears when it lets go.
    #[test]
    fn a_browser_session_journal_held_elsewhere_is_still_retried() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("browser-sessions.jsonl");
        let _held = crate::browser_session::adapters::PersistentSessions::open(&path, 100).unwrap();
        let Err(cause) = crate::browser_session::adapters::PersistentSessions::open(&path, 100)
        else {
            panic!("the journal opened twice");
        };
        let error = RunError::opening_browser_sessions(&path, cause);
        assert!(matches!(error, RunError::Authentication(_)), "{error}");
        assert_eq!(
            super::super::restart::restart(&error),
            super::super::restart::Restart::Worthwhile
        );
    }

    #[test]
    fn display_environment_error() {
        let error = RunError::Environment(EnvironmentError::Empty { variable: HOST });
        assert!(error.to_string().contains(HOST));
    }
}
