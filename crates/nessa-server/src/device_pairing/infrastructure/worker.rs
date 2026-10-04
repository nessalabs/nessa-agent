//! Original physical worker failures retain typed termination meaning.
use nessa_auth::application::pairing::PairingWorkerFault;
use tokio::task::JoinError;
pub(super) fn worker_fault(error: JoinError) -> PairingWorkerFault {
    if error.is_panic() {
        PairingWorkerFault::Panic
    } else {
        PairingWorkerFault::Cancelled
    }
}
