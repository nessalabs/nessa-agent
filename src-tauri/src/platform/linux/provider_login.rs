use crate::provider_authentication::{LoginFailure, Provider, ProviderLogin};

/// This platform does not yet implement a provider CLI login launcher.
pub struct LinuxProviderLogin;

impl ProviderLogin for LinuxProviderLogin {
    fn available(&self) -> bool {
        false
    }
    fn open(&self, _: Provider) -> Result<(), LoginFailure> {
        Err(LoginFailure::Unavailable)
    }
}

#[cfg(test)]
#[path = "../../../tests/platform/linux/provider_login.rs"]
mod tests;
