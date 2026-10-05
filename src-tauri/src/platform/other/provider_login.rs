use crate::provider_authentication::{LoginFailure, Provider, ProviderLogin};

/// This platform does not yet implement a provider CLI login launcher.
pub struct OtherProviderLogin;

impl ProviderLogin for OtherProviderLogin {
    fn available(&self) -> bool {
        false
    }
    fn open(&self, _: Provider) -> Result<(), LoginFailure> {
        Err(LoginFailure::Unavailable)
    }
}

#[cfg(test)]
#[path = "../../../tests/platform/other/provider_login.rs"]
mod tests;
