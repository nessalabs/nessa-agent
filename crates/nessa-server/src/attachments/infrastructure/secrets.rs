use crate::attachments::application::{SecretsUnavailable, TicketSecrets};

/// Ticket secrets from the operating system's random source.
pub struct OsTicketSecrets;
impl TicketSecrets for OsTicketSecrets {
    fn fresh(&self) -> Result<[u8; 32], SecretsUnavailable> {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| SecretsUnavailable)?;
        Ok(bytes)
    }
}
