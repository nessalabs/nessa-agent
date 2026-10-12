//! How a machine pairs as an environment, once (issue #708, part K2): the
//! code is sent only over no pairing, with the gateway's address recorded
//! first, and a container's every later start finds the pairing it made
//! instead of sending a used code again.
//!
//! ```text
//! enroll_once ──▶ saved? ──▶ nothing ──▶ read the code ──▶ enroll
//!                        ──▶ pending or approved ──▶ recorded address?
//!                              ──▶ this gateway's ──▶ before: send nothing
//!                              ──▶ another, or none ──▶ paired elsewhere
//! enroll ──▶ saved nothing? ──▶ record this address ──▶ send the code
//! ```
//!
//! Arrows are steps, in order. The pairing and the code are ports, so every
//! answer, failures included, can be given by a test.
use super::approval::{EnvironmentPairing, Saved};
use std::future::Future;

/// A pairing that can enroll, and that keeps the address of the gateway it
/// enrolled with.
pub(crate) trait EnrollingPairing: EnvironmentPairing {
    /// The owner's code, as the gateway takes it.
    type Code: Send;
    /// The address of the gateway this pairing enrolls with and dials.
    fn address(&self) -> &str;
    /// The address recorded when this machine enrolled; `None` when none
    /// was. An error is a record that cannot be read.
    fn recorded_address(&self) -> impl Future<Output = Result<Option<String>, String>> + Send;
    /// Record [`Self::address`] as the one enrolled with, replacing whatever
    /// an earlier pairing left.
    fn record_address(&self) -> impl Future<Output = Result<(), String>> + Send;
    /// Send `code` to the gateway to pair; the message says what to do when
    /// it does not.
    fn send_code(&self, code: Self::Code) -> impl Future<Output = Result<(), String>> + Send;
}

/// Where the owner's code is kept, as a file a container mounts.
pub(crate) trait EnrollmentCode<C> {
    /// The code; the message says why there is none.
    fn read(&self) -> Result<C, String>;
}

/// What [`enroll_once`] did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum EnrolledOnce {
    /// Nothing was paired here: it enrolled with the code.
    Now,
    /// It was already paired with this gateway, approved or not: the code,
    /// used then, was neither read nor sent.
    Before,
}

/// Why [`enroll_once`] did not enroll.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NotEnrolled {
    /// Paired already with the gateway at this recorded address, or with
    /// one it did not record (`None`): never dialed in its place.
    PairedElsewhere(Option<String>),
    /// The code, the pairing or the gateway failed: the reason says which.
    Failed(String),
}

/// Enroll `pairing` with `code`, recording its address first when nothing
/// is paired: a reply lost after the gateway took the code still finds that
/// address on the next start, and a pairing that exists keeps its own.
///
/// # Errors
/// The pairing could not be read or recorded, or the gateway did not pair
/// this machine.
pub(crate) async fn enroll<P: EnrollingPairing>(pairing: &P, code: P::Code) -> Result<(), String> {
    if matches!(pairing.saved().await?, Saved::Nothing) {
        pairing.record_address().await?;
    }
    pairing.send_code(code).await
}

/// Enroll with the code `code` holds when nothing is paired here, as a
/// container's entrypoint does on every start; paired already with this
/// gateway, nothing is read or sent.
///
/// # Errors
/// [`NotEnrolled`]: paired with another gateway, or enrolling failed.
pub(crate) async fn enroll_once<P: EnrollingPairing>(
    pairing: &P,
    code: &impl EnrollmentCode<P::Code>,
) -> Result<EnrolledOnce, NotEnrolled> {
    match pairing.saved().await.map_err(NotEnrolled::Failed)? {
        Saved::Nothing => {
            let code = code.read().map_err(NotEnrolled::Failed)?;
            enroll(pairing, code).await.map_err(NotEnrolled::Failed)?;
            Ok(EnrolledOnce::Now)
        }
        Saved::Pending | Saved::Approved(_) => {
            match pairing
                .recorded_address()
                .await
                .map_err(NotEnrolled::Failed)?
            {
                Some(address) if address == pairing.address() => Ok(EnrolledOnce::Before),
                recorded => Err(NotEnrolled::PairedElsewhere(recorded)),
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/env_serve/enrollment.rs"]
mod tests;
