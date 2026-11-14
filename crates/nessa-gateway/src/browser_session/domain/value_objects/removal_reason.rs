/// Why an opaque browser session left authoritative storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemovalReason {
    /// A sign-in committed after its HTTP waiter reached the response deadline.
    AbandonedLogin,
    /// The verified browser caller explicitly signed out.
    SignOut,
    /// The rolling idle deadline elapsed.
    IdleExpired,
    /// The referenced credential was revoked after sign-in.
    CredentialRevoked,
    /// The current registry credential reached its explicit deadline.
    CredentialExpired,
    /// The credential's current membership is no longer active.
    InactiveMembership,
    /// The referenced credential no longer agrees with current authority.
    IdentityMismatch,
    /// Current credential state is no longer valid.
    InvalidCredential,
}
