//! A noninteractive read from the macOS login keychain.
//!
//! `kSecUseAuthenticationUIFail` is attached to this query only. Security then
//! returns an error when reading would need UI, which lets a background gateway
//! report the source as unavailable without prompting or changing global
//! keychain interaction state.

use crate::agents::application::AgentCredentialFailure;

pub(super) fn read(
    service: &str,
    account: &str,
) -> Result<Option<Vec<u8>>, AgentCredentialFailure> {
    crate::secrets::read_macos(service, account).map_err(|failure| match failure {
        crate::secrets::SecretReadFailure::Unavailable => AgentCredentialFailure::Unavailable,
        crate::secrets::SecretReadFailure::Invalid => AgentCredentialFailure::Invalid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use security_framework::passwords::{delete_generic_password, set_generic_password};
    use std::{process, time::SystemTime};

    /// This exercises the real login keychain and therefore stays explicit.
    /// The account is unique, cleanup runs before the assertion, and neither
    /// operation puts the secret in an argument or diagnostic.
    #[test]
    #[ignore = "writes a disposable item to the current macOS login keychain"]
    fn reads_a_disposable_generic_password_through_the_noninteractive_boundary() {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let service = "so.nessa.agent-credentials.boundary-test";
        let account = format!("{}-{nonce}", process::id());
        set_generic_password(service, &account, b"disposable-secret").unwrap();

        let answer = read(service, &account);
        delete_generic_password(service, &account).unwrap();

        assert_eq!(
            answer.unwrap().as_deref(),
            Some(b"disposable-secret".as_slice())
        );
    }
}
