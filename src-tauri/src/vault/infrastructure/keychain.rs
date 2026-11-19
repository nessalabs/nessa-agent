//! macOS fingerprint unlock: the passphrase lives in the login keychain, and
//! LocalAuthentication's Touch ID prompt guards every read of it.
//!
//! Why not a keychain item with a biometric access control: that needs the
//! data-protection keychain, which needs a `keychain-access-groups`
//! entitlement and a provisioning profile this Developer ID app does not
//! carry (and a local ad-hoc build cannot). So the item is an ordinary login
//! keychain item, whose access list trusts only this app's signature, and the
//! app shows the fingerprint prompt before reading it. The enrolled
//! fingerprints' state hash is saved beside the passphrase; a different hash
//! at unlock means fingers were added or removed, and the copy is dropped, as
//! `biometryCurrentSet` would.
use crate::vault::application::biometrics::{BiometricFailure, BiometricStore};
use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_foundation::{NSError, NSString};
use objc2_local_authentication::{LAContext, LAError, LAPolicy};
use security_framework::{
    item::{ItemClass, ItemSearchOptions},
    passwords::{delete_generic_password, get_generic_password, set_generic_password},
};
use std::sync::mpsc;
use zeroize::Zeroizing;

/// The keychain item's service, shown as its name in Keychain Access.
const SERVICE: &str = "so.nessa.vault";
const POLICY: LAPolicy = LAPolicy::DeviceOwnerAuthenticationWithBiometrics;
/// Finishes the system's sentence: "Nessa is trying to unlock your vault."
const REASON: &str = "unlock your vault";
/// The prompt's second button, which leaves the passphrase form open.
const FALLBACK: &str = "Enter Passphrase";

/// One saved passphrase per gateway namespace, as the vault itself is.
pub struct TouchIdKeychain {
    account: String,
}

impl TouchIdKeychain {
    /// `account` is the credential namespace's `vault-touch-id` item.
    pub fn new(account: String) -> Self {
        Self { account }
    }
}

impl BiometricStore for TouchIdKeychain {
    fn available(&self) -> bool {
        let context = unsafe { LAContext::new() };
        unsafe { context.canEvaluatePolicy_error(POLICY) }.is_ok()
    }

    fn saved(&self) -> bool {
        // Attributes only: no secret is decrypted, so no prompt can appear.
        ItemSearchOptions::new()
            .class(ItemClass::generic_password())
            .service(SERVICE)
            .account(&self.account)
            .load_attributes(true)
            .limit(1)
            .search()
            .is_ok_and(|found| !found.is_empty())
    }

    fn save(&self, passphrase: &str) -> Result<(), BiometricFailure> {
        let context = unsafe { LAContext::new() };
        unsafe { context.canEvaluatePolicy_error(POLICY) }
            .map_err(|_| BiometricFailure::Unavailable)?;
        // A successful preflight fills in the enrolled fingerprints' state.
        let state = domain_state(&context).ok_or(BiometricFailure::Unavailable)?;
        let secret = encode(&state, passphrase)?;
        set_generic_password(SERVICE, &self.account, &secret)
            .map_err(|_| BiometricFailure::Unavailable)
    }

    fn release(&self) -> Result<Zeroizing<String>, BiometricFailure> {
        if !self.saved() {
            return Err(BiometricFailure::Missing);
        }
        let context = unsafe { LAContext::new() };
        unsafe { context.setLocalizedFallbackTitle(Some(&NSString::from_str(FALLBACK))) };
        let (answer, answered) = mpsc::sync_channel(1);
        let reply = RcBlock::new(move |success: Bool, error: *mut NSError| {
            let outcome = if success.as_bool() {
                Ok(())
            } else {
                // SAFETY: LocalAuthentication passes a valid NSError or nil.
                Err(match unsafe { error.as_ref() }.map(|error| error.code()) {
                    Some(code)
                        if [
                            LAError::UserCancel,
                            LAError::UserFallback,
                            LAError::SystemCancel,
                            LAError::AppCancel,
                        ]
                        .iter()
                        .any(|cancel| cancel.0 == code) =>
                    {
                        BiometricFailure::Cancelled
                    }
                    _ => BiometricFailure::Unavailable,
                })
            };
            let _ = answer.send(outcome);
        });
        // SAFETY: the block only moves a `SyncSender`, which is `Send`; the
        // context stays alive until the reply arrives, below.
        unsafe {
            context.evaluatePolicy_localizedReason_reply(
                POLICY,
                &NSString::from_str(REASON),
                &reply,
            )
        };
        answered
            .recv()
            .map_err(|_| BiometricFailure::Unavailable)??;
        let now = domain_state(&context);
        let secret = Zeroizing::new(
            get_generic_password(SERVICE, &self.account)
                .map_err(|_| BiometricFailure::Unavailable)?,
        );
        let Some((state, passphrase)) = decode(&secret) else {
            self.forget();
            return Err(BiometricFailure::Missing);
        };
        if now.as_deref() != Some(state) {
            self.forget();
            return Err(BiometricFailure::Changed);
        }
        Ok(Zeroizing::new(passphrase.to_owned()))
    }

    fn forget(&self) {
        let _ = delete_generic_password(SERVICE, &self.account);
    }
}

fn domain_state(context: &LAContext) -> Option<Vec<u8>> {
    // Deprecated in favour of `domainState`, which needs macOS 15; this app
    // supports 13.5. Its value may change once across a major OS update,
    // which costs one typed passphrase.
    #[allow(deprecated)]
    let data = unsafe { context.evaluatedPolicyDomainState() }?;
    Some(data.to_vec())
}

/// `state length (2 bytes, big-endian) | state | passphrase (UTF-8)`.
fn encode(state: &[u8], passphrase: &str) -> Result<Zeroizing<Vec<u8>>, BiometricFailure> {
    let length = u16::try_from(state.len()).map_err(|_| BiometricFailure::Unavailable)?;
    let mut secret = Zeroizing::new(Vec::with_capacity(2 + state.len() + passphrase.len()));
    secret.extend_from_slice(&length.to_be_bytes());
    secret.extend_from_slice(state);
    secret.extend_from_slice(passphrase.as_bytes());
    Ok(secret)
}

fn decode(secret: &[u8]) -> Option<(&[u8], &str)> {
    let length = usize::from(u16::from_be_bytes([*secret.first()?, *secret.get(1)?]));
    let state = secret.get(2..2 + length)?;
    let passphrase = std::str::from_utf8(secret.get(2 + length..)?).ok()?;
    (!state.is_empty() && !passphrase.is_empty()).then_some((state, passphrase))
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};

    #[test]
    fn the_saved_secret_reads_back_its_state_and_passphrase() {
        let secret = encode(b"fingers", "disposable test words").unwrap();
        assert_eq!(
            decode(&secret),
            Some((&b"fingers"[..], "disposable test words"))
        );
    }

    #[test]
    fn a_cut_or_empty_secret_reads_as_nothing() {
        let secret = encode(b"fingers", "words").unwrap();
        assert_eq!(decode(&secret[..5]), None);
        assert_eq!(decode(&encode(b"", "words").unwrap()), None);
        assert_eq!(decode(&encode(b"fingers", "").unwrap()), None);
        assert_eq!(decode(&[]), None);
    }
}
