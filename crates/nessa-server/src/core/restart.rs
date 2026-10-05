//! Whether starting this process again could end any differently.
//!
//! The packaged gateway is supervised by launchd, which is told to keep it
//! running. Keeping a crashed server running is the point of that; retrying a
//! registry this build cannot read, every five seconds, for as long as the user
//! is logged in, is not. The two are told apart here, once, on the typed
//! failure — never on the words the failure happens to print.
//!
//! launchd can express "restart unless the process exited successfully"
//! (`KeepAlive: { SuccessfulExit: false }`) and nothing finer: there is no
//! condition on *which* non-zero code. So a failure that retrying cannot fix
//! ends the process with a zero status, which is the only sentence launchd
//! understands as "do not start me again", and writes down what actually
//! happened for the desktop host to read — see [`super::startup_failure`].
//!
//! Only failures that provably cannot improve are treated that way. Everything
//! else keeps the behaviour it had, because a service that retries too often is
//! a worse bug than one that retries when it did not need to, but a service
//! that gives up on a failure that would have cleared is worse than both.
use crate::device_pairing::infrastructure::{GatewayIdentityError, PairingRuntimeError};
use nessa_auth::adapters::local::LocalStoreError;
use nessa_auth::application::pairing::{PairingStoreError, PrivateStateError};

use super::{error::NativeFailure, RunError};

/// What a fatal failure says about being started again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Restart {
    /// The cause can clear without anyone doing anything: a port the process
    /// holding it is about to release, a registry lock held by a gateway that
    /// is still shutting down, a disk that was busy. launchd tries again.
    Worthwhile,
    /// The next attempt reads the same configuration, the same registry and the
    /// same runtime, and reaches the same answer. It is reported once and left
    /// alone until someone changes something.
    Pointless,
}

/// Exhaustive by construction, like the exit-code table beside it: a new fatal
/// failure does not compile until someone has said whether retrying it helps.
pub(super) fn restart(error: &RunError) -> Restart {
    match error {
        // Configuration is read once, from the launchd definition and the
        // environment it fixes. Nothing rereads differently five seconds later.
        RunError::Environment(_) => Restart::Pointless,
        // The same for `config.json`: its refused contents are read again
        // unchanged (design row S2).
        RunError::RuntimeConfig(_) => Restart::Pointless,
        // Contents this build cannot make sense of, including a registry
        // written by a schema it does not know. Reading them again is reading
        // the same bytes.
        RunError::Registry(failure)
            if matches!(
                failure.primary(),
                LocalStoreError::Corrupt
                    | LocalStoreError::InvalidRegistry { .. }
                    | LocalStoreError::Capacity
            ) =>
        {
            Restart::Pointless
        }
        // A registry another process is holding. That holder can let go — an
        // upgrade's outgoing gateway is still finishing while its replacement
        // starts — so this is exactly the failure launchd's retry is for.
        RunError::Registry(failure) if matches!(failure.primary(), LocalStoreError::Locked) => {
            Restart::Worthwhile
        }
        // A prepared runtime that is missing, unreadable, or not the one this
        // registration was fingerprinted against. Only a new registration
        // changes any of that.
        RunError::Runtime(_) => Restart::Pointless,
        // Another version, or not a database: the same bytes next time.
        RunError::Dataset(_) => Restart::Pointless,
        // The journal is gone and an enrollment still owes a fence. Creating
        // an empty one would make that fence a missing receiver, and the
        // file does not reappear on its own (design row RJ2).
        RunError::ReceiverJournal(_) => Restart::Pointless,
        // The command line named nothing this build can run. Under launchd
        // that command line is this installation's own plist, which the next
        // attempt reads unchanged, so retrying is the relaunch loop and not a
        // recovery.
        RunError::Usage(_) => Restart::Pointless,
        // Native pairing state that contradicts itself, or a missing key with
        // enrollment history: the same files refuse the same way next time
        // (design row S16). Held locks, taken ports and the rest fall through.
        RunError::Native(
            NativeFailure::PrivateState(PrivateStateError::Corrupt | PrivateStateError::Conflict)
            | NativeFailure::Identity(
                GatewayIdentityError::PrivateState(
                    PrivateStateError::Corrupt | PrivateStateError::Conflict,
                )
                | GatewayIdentityError::Registry(
                    PairingStoreError::GatewayKeyHistoryExists
                    | PairingStoreError::PrivateState(
                        PrivateStateError::Corrupt | PrivateStateError::Conflict,
                    ),
                ),
            ),
        ) => Restart::Pointless,
        // An ended enrollment's cleanup that the one recurrence classifier
        // says will fail the same way: startup would fail forever (design rows
        // S8, S16). Approval's activationStopped asks the same classifier.
        RunError::Native(NativeFailure::Open(PairingRuntimeError::Cleanup(error)))
            if error.recurs() =>
        {
            Restart::Pointless
        }
        // Everything below is either transient by nature or carries no typed
        // cause to judge — an opaque message is not evidence of permanence, and
        // guessing wrong here strands a gateway that would have started.
        RunError::Registry(_)
        | RunError::Authentication(_)
        | RunError::Agent(_)
        | RunError::Bind { .. }
        | RunError::Serve(_)
        | RunError::Shutdown(_)
        // The rest of native pairing can clear: a held private state, a taken
        // port, a failed listener, unavailable storage.
        | RunError::Native(_) => Restart::Worthwhile,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_pairing::application::{CleanupError, ReceiverError};
    use crate::env::{EnvironmentError, HOST};
    use nessa_auth::domain::pairing::PairingError;
    use std::io::{Error, ErrorKind};

    /// The failure this issue was reported for: a registry this build cannot
    /// read, retried every five seconds until the data directory was repaired
    /// by hand.
    #[test]
    fn a_registry_this_build_cannot_read_is_not_worth_starting_for_again() {
        for error in [
            RunError::registry(LocalStoreError::Corrupt, None),
            RunError::registry(LocalStoreError::Capacity, None),
            RunError::Environment(EnvironmentError::Empty { variable: HOST }),
            RunError::RuntimeConfig("unknown field native.tls".into()),
            RunError::Runtime("missing bundled runtime file".into()),
            // The command line is read again unchanged, so the next attempt
            // fails on the same words.
            RunError::Usage("unknown command".into()),
            // A store this build cannot read is the same file next time.
            RunError::opening_browser_sessions(
                std::path::Path::new("browser-sessions.jsonl"),
                crate::browser_session::adapters::JournalOpenError::Unreadable {
                    line: Some(1),
                    problem: "is not a journal record",
                },
            ),
            // Native pairing state that refuses the same way next time (row S16).
            RunError::Native(NativeFailure::Identity(GatewayIdentityError::Registry(
                PairingStoreError::GatewayKeyHistoryExists,
            ))),
            RunError::Native(NativeFailure::Identity(GatewayIdentityError::PrivateState(
                PrivateStateError::Corrupt,
            ))),
            RunError::Native(NativeFailure::Identity(GatewayIdentityError::PrivateState(
                PrivateStateError::Conflict,
            ))),
            RunError::Native(NativeFailure::PrivateState(PrivateStateError::Corrupt)),
            // An ended enrollment's cleanup that refuses the same way (row S8).
            RunError::Native(NativeFailure::Open(PairingRuntimeError::Cleanup(
                CleanupError::Receiver(ReceiverError::Missing),
            ))),
            RunError::Native(NativeFailure::Open(PairingRuntimeError::Cleanup(
                CleanupError::Receiver(ReceiverError::Conflict),
            ))),
            RunError::Native(NativeFailure::Open(PairingRuntimeError::Cleanup(
                CleanupError::Enrollment(PairingStoreError::Domain(PairingError::Conflict)),
            ))),
            RunError::Native(NativeFailure::PrivateState(PrivateStateError::Conflict)),
            // The same private state, met by the first key publication.
            RunError::Native(NativeFailure::Identity(GatewayIdentityError::Registry(
                PairingStoreError::PrivateState(PrivateStateError::Corrupt),
            ))),
            RunError::Native(NativeFailure::Identity(GatewayIdentityError::Registry(
                PairingStoreError::PrivateState(PrivateStateError::Conflict),
            ))),
            RunError::ReceiverJournal(crate::core::MissingReceiverJournal::new(
                std::path::PathBuf::from("receiver-access/receiver-access.sqlite3"),
            )),
        ] {
            assert_eq!(restart(&error), Restart::Pointless, "{error}");
        }
    }

    /// A lock is held by someone, and someone lets go. An upgrade's outgoing
    /// gateway still holds this stage's registry while its replacement starts,
    /// and that replacement must be allowed to try again.
    #[test]
    fn a_failure_that_can_clear_on_its_own_is_still_retried() {
        for error in [
            RunError::registry(LocalStoreError::Locked, None),
            RunError::registry(
                LocalStoreError::Io(Error::from(ErrorKind::PermissionDenied)),
                None,
            ),
            RunError::Bind {
                addr: "127.0.0.1:7420".into(),
                source: Error::from(ErrorKind::AddrInUse),
            },
            RunError::Serve(Error::from(ErrorKind::BrokenPipe)),
            RunError::Shutdown(None),
            // No typed cause to judge: the message is prose, and prose is not
            // evidence that the next attempt would fail the same way.
            RunError::Authentication("setup".into()),
            RunError::Agent("provider".into()),
            // Native pairing's held private state, taken port, or failed
            // listener can each clear.
            RunError::Native(NativeFailure::Listener(ErrorKind::InvalidInput)),
            RunError::Native(NativeFailure::Bind {
                address: "127.0.0.1:47650".parse().unwrap(),
                source: Error::from(ErrorKind::AddrInUse),
            }),
            RunError::Native(NativeFailure::PrivateState(PrivateStateError::Locked)),
            // A receiver or registry that is only unavailable can clear.
            RunError::Native(NativeFailure::Open(PairingRuntimeError::Cleanup(
                CleanupError::Receiver(ReceiverError::Unavailable),
            ))),
            RunError::Native(NativeFailure::Open(PairingRuntimeError::Cleanup(
                CleanupError::Enrollment(PairingStoreError::StageOccupied),
            ))),
            RunError::Native(NativeFailure::Identity(GatewayIdentityError::PrivateState(
                PrivateStateError::Unavailable,
            ))),
            RunError::Native(NativeFailure::Identity(GatewayIdentityError::Registry(
                PairingStoreError::Unavailable,
            ))),
            RunError::Native(NativeFailure::Identity(GatewayIdentityError::Registry(
                PairingStoreError::PrivateState(PrivateStateError::Locked),
            ))),
            RunError::Native(NativeFailure::Directory(Error::from(
                ErrorKind::PermissionDenied,
            ))),
        ] {
            assert_eq!(restart(&error), Restart::Worthwhile, "{error}");
        }
    }

    /// Row A11/S8: approval's activationStopped and startup's restart policy
    /// derive from one classifier, so they agree for every cleanup failure.
    #[test]
    fn one_classifier_decides_retry_and_relaunch() {
        use crate::device_pairing::application::ActivationError;
        use nessa_auth::application::pairing::PairingWorkerFault;
        let stores = [
            PairingStoreError::StageOccupied,
            PairingStoreError::GatewayKeyHistoryExists,
            PairingStoreError::PrivateState(PrivateStateError::Corrupt),
            PairingStoreError::PrivateState(PrivateStateError::Conflict),
            PairingStoreError::PrivateState(PrivateStateError::Locked),
            PairingStoreError::PrivateState(PrivateStateError::Unavailable),
            PairingStoreError::PrivateState(PrivateStateError::Uncertain),
            PairingStoreError::WorkerFault(PairingWorkerFault::Panic),
            PairingStoreError::WorkerFault(PairingWorkerFault::Cancelled),
            PairingStoreError::StaleRevision,
            PairingStoreError::NotFound,
            PairingStoreError::Unavailable,
        ]
        .into_iter()
        .chain(
            [
                PairingError::Invalid,
                PairingError::Conflict,
                PairingError::Ineligible,
                PairingError::WrongActor,
                PairingError::Expired,
                PairingError::Capacity,
                PairingError::AttemptsExhausted,
                PairingError::AvailableSlotOccupied,
                PairingError::StaleGeneration,
            ]
            .map(PairingStoreError::Domain),
        )
        .map(CleanupError::Enrollment);
        let receivers = [
            ReceiverError::Unavailable,
            ReceiverError::Conflict,
            ReceiverError::Missing,
            ReceiverError::Exhausted,
            ReceiverError::NotPaired,
        ]
        .map(CleanupError::Receiver);
        for error in stores.chain(receivers) {
            let relaunch = restart(&RunError::Native(NativeFailure::Open(
                PairingRuntimeError::Cleanup(error),
            )));
            let retry = ActivationError::Cleanup(error).retryable();
            assert_eq!(relaunch == Restart::Worthwhile, retry, "{error:?}");
            assert_eq!(retry, !error.recurs(), "{error:?}");
        }
        // A clock behind the record (Invalid) clears on its own; capacity and
        // the record's own state refuse again.
        for (error, recurs) in [
            (PairingError::Invalid, false),
            (PairingError::Capacity, true),
            (PairingError::AvailableSlotOccupied, true),
            (PairingError::Conflict, true),
            (PairingError::Ineligible, true),
            (PairingError::StaleGeneration, true),
        ] {
            let cleanup = CleanupError::Enrollment(PairingStoreError::Domain(error));
            assert_eq!(cleanup.recurs(), recurs, "{error:?}");
        }
        // An owner who can sign in again can approve again; a denial cannot.
        use crate::device_pairing::application::OwnerError;
        use nessa_auth::application::ports::AccessError;
        for (error, retryable) in [
            (AccessError::CredentialExpired, true),
            (AccessError::InactiveMembership, true),
            (AccessError::Unavailable, true),
            (AccessError::Denied, false),
            (AccessError::CredentialRevoked, false),
        ] {
            assert_eq!(
                ActivationError::Owner(OwnerError::Authorization(error)).retryable(),
                retryable,
                "{error:?}"
            );
        }
    }
}
