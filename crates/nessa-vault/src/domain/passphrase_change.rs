use super::Failure;

/// What the encrypted repository can prove about a passphrase replacement.
/// An uncertain result must not be retried automatically or treated as unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangePublication {
    /// This operation did not attempt replacement; it does not attest other writers.
    Unchanged(Failure),
    /// New ciphertext and its required local durability receipts were verified.
    Committed,
    /// Replacement was attempted but its completed durability could not be confirmed.
    Uncertain(Failure),
}
