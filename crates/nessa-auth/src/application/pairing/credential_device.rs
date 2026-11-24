//! What a credential was issued to, read from how it was issued.
//!
//! A credential is issued once and its proof never changes after: a bearer
//! token, or a device key bound to one pairing whose consent class says what
//! the device is. Revoking or expiring a credential does not change what it
//! was issued to, so this answer is the same for an old turn as for a new one.
use crate::{application::ports::PortFuture, domain::CredentialId};

/// What a credential was issued to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialDevice {
    /// The owner's phone, paired for read and write.
    Phone,
    /// Another of the owner's own devices, paired to read.
    PairedDevice,
    /// A peer gateway or worker, paired as itself.
    Peer,
    /// A bearer token this gateway wrote to a file on the computer it runs
    /// on: the desktop app, the command line, or a provisioned surface.
    Token,
}

/// Reads what a credential was issued to.
pub trait CredentialDeviceSource: Send + Sync {
    /// `None` when this gateway never issued `credential`.
    fn credential_device<'a>(
        &'a self,
        credential: &'a CredentialId,
    ) -> PortFuture<'a, Option<CredentialDevice>>;
}
