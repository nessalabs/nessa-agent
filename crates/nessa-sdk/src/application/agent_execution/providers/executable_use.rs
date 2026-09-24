//! Keeps executable-use ownership attached to each ACP process generation.
#![deny(missing_docs)]

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

/// A failure to durably admit or release one executable-use generation.
///
/// The message is diagnostic only. Callers must use the failed operation and
/// the still-owned guard to decide whether another release attempt is needed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutableUseError {
    message: String,
}

impl ExecutableUseError {
    /// Creates a diagnostic for a failed durable use operation.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ExecutableUseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ExecutableUseError {}

/// A failed executable-use admission and any exact pre-spawn generation it created.
///
/// [`Self::into_parts`] returns a guard only when admission created a generation
/// identity before failing. Its durable evidence may be partial or have an
/// uncertain acknowledgement. The guard owns that known-never-spawned
/// generation and may finish its release without process cleanup. A failure
/// without a guard occurred before any generation existed.
pub struct ExecutableUseAdmissionFailure {
    error: ExecutableUseError,
    guard: Option<Box<dyn ExecutableUseGuard>>,
}

impl ExecutableUseAdmissionFailure {
    /// Creates a failure that occurred before a process generation was created.
    pub fn before_generation(error: ExecutableUseError) -> Self {
        Self { error, guard: None }
    }

    /// Creates a failure that still owns an exact, known-never-spawned generation.
    ///
    /// The caller must preserve `guard` until its explicit release succeeds.
    /// Dropping it leaves the durable admission unresolved.
    pub fn with_generation(error: ExecutableUseError, guard: Box<dyn ExecutableUseGuard>) -> Self {
        Self {
            error,
            guard: Some(guard),
        }
    }

    /// Returns the diagnostic failure without consuming retry ownership.
    pub fn error(&self) -> &ExecutableUseError {
        &self.error
    }

    /// Separates the diagnostic from an exact pre-spawn generation owner.
    ///
    /// A returned guard may be released immediately because no spawn was
    /// attempted after this admission failure. If release fails, the same guard
    /// retains retry ownership.
    pub fn into_parts(self) -> (ExecutableUseError, Option<Box<dyn ExecutableUseGuard>>) {
        (self.error, self.guard)
    }
}

impl fmt::Debug for ExecutableUseAdmissionFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExecutableUseAdmissionFailure")
            .field("error", &self.error)
            .field("owns_generation", &self.guard.is_some())
            .finish()
    }
}

impl fmt::Display for ExecutableUseAdmissionFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(formatter)
    }
}

impl std::error::Error for ExecutableUseAdmissionFailure {}

/// Installation-independent authority that admits use of one executable.
///
/// Implementations may durably record each process generation. A successful
/// admission returns the only handle that can acknowledge that generation as
/// no longer in use. Dropping either this authority or a returned guard does
/// not acknowledge release.
pub trait ExecutableUse: Send + Sync {
    /// Returns the executable path governed by this authority.
    fn executable(&self) -> &Path;

    /// Durably admits one process generation before its spawn is attempted.
    fn admit(&self) -> Result<Box<dyn ExecutableUseGuard>, ExecutableUseAdmissionFailure>;
}

/// One admitted executable-use generation.
///
/// Release is explicit and retryable. It is valid only after the caller proves
/// that the generation never spawned or that all of its processes were cleaned
/// up. Dropping the guard conservatively leaves the admission unresolved.
pub trait ExecutableUseGuard: Send {
    /// Durably acknowledges that this generation can no longer use the executable.
    ///
    /// A failure leaves the guard owned so the caller can retry without
    /// repeating already-confirmed process cleanup.
    fn release(&mut self) -> Result<(), ExecutableUseError>;
}

/// Immutable executable path paired with its process-use authority.
///
/// Clone this value when multiple launch configurations share the same
/// authority. Each launch still obtains a distinct guard with [`Self::admit`].
#[derive(Clone)]
pub struct ExecutableUseSnapshot {
    executable: PathBuf,
    authority: Arc<dyn ExecutableUse>,
}

impl fmt::Debug for ExecutableUseSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExecutableUseSnapshot")
            .field("executable", &self.executable)
            .finish_non_exhaustive()
    }
}

impl ExecutableUseSnapshot {
    /// Creates a snapshot after confirming the path agrees with the authority.
    ///
    /// Returns an error when `executable` is not exactly the path governed by
    /// `authority`; no use is admitted in that case.
    pub fn new(
        executable: PathBuf,
        authority: Arc<dyn ExecutableUse>,
    ) -> Result<Self, ExecutableUseError> {
        if executable != authority.executable() {
            return Err(ExecutableUseError::new(
                "executable path does not match its use authority",
            ));
        }
        Ok(Self {
            executable,
            authority,
        })
    }

    /// Creates a snapshot for a bundled or user-selected executable that has
    /// no managed-installation reclamation policy.
    pub fn unmanaged(executable: PathBuf) -> Self {
        let authority = Arc::new(UnmanagedExecutableUse {
            executable: executable.clone(),
        });
        Self {
            executable,
            authority,
        }
    }

    /// Returns the executable path governed by this snapshot.
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// Durably admits one process generation before spawn.
    pub fn admit(&self) -> Result<Box<dyn ExecutableUseGuard>, ExecutableUseAdmissionFailure> {
        self.authority.admit()
    }
}

struct UnmanagedExecutableUse {
    executable: PathBuf,
}

impl ExecutableUse for UnmanagedExecutableUse {
    fn executable(&self) -> &Path {
        &self.executable
    }

    fn admit(&self) -> Result<Box<dyn ExecutableUseGuard>, ExecutableUseAdmissionFailure> {
        Ok(Box::new(UnmanagedExecutableUseGuard))
    }
}

struct UnmanagedExecutableUseGuard;

impl ExecutableUseGuard for UnmanagedExecutableUseGuard {
    fn release(&mut self) -> Result<(), ExecutableUseError> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../tests/application/agent_execution/providers/executable_use.rs"]
mod tests;
