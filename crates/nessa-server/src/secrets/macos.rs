//! Per-operation noninteractive Keychain queries. App-owned entries use the
//! existing user-default keychain explicitly; no keychain is created or unlocked.
use super::SecretUnavailable;
use core_foundation::{
    array::CFArray,
    base::{TCFType, ToVoid},
    boolean::CFBoolean,
    data::CFData,
    dictionary::CFMutableDictionary,
    string::CFString,
};
use core_foundation_sys::{
    base::{CFGetTypeID, CFRelease, CFTypeRef},
    string::CFStringRef,
};
use security_framework::os::macos::keychain::{SecKeychain, SecPreferencesDomain};
use security_framework_sys::{
    base::{errSecItemNotFound, SecKeychainRef},
    item::{
        kSecAttrAccount, kSecAttrService, kSecClass, kSecClassGenericPassword, kSecMatchSearchList,
        kSecReturnData, kSecUseAuthenticationUI, kSecUseKeychain, kSecValueData,
    },
    keychain_item::{SecItemAdd, SecItemCopyMatching, SecItemDelete, SecItemUpdate},
};
use std::{ffi::c_void, ptr};

type Query = CFMutableDictionary<*const c_void, *const c_void>;
#[derive(Debug)]
pub(crate) enum SecretReadFailure {
    Unavailable,
    Invalid,
}
#[link(name = "Security", kind = "framework")]
extern "C" {
    static kSecUseAuthenticationUIFail: CFStringRef;
    fn SecKeychainGetStatus(keychain: SecKeychainRef, status: *mut u32) -> i32;
    #[cfg(test)]
    fn SecKeychainLock(keychain: SecKeychainRef) -> i32;
    #[cfg(test)]
    fn SecKeychainDelete(keychain: SecKeychainRef) -> i32;
}
fn query(service: &str, account: &str) -> Query {
    let mut query = Query::new();
    // CFDictionary retains these CF objects with kCFType callbacks.
    unsafe {
        query.add(&kSecClass.to_void(), &kSecClassGenericPassword.to_void());
        query.add(
            &kSecAttrService.to_void(),
            &CFString::new(service).to_void(),
        );
        query.add(
            &kSecAttrAccount.to_void(),
            &CFString::new(account).to_void(),
        );
        query.add(
            &kSecUseAuthenticationUI.to_void(),
            &kSecUseAuthenticationUIFail.to_void(),
        );
    }
    query
}
fn read_query(mut query: Query) -> Result<Option<Vec<u8>>, SecretReadFailure> {
    unsafe {
        query.add(
            &kSecReturnData.to_void(),
            &CFBoolean::true_value().to_void(),
        );
        let mut result: CFTypeRef = ptr::null();
        let status = SecItemCopyMatching(query.as_concrete_TypeRef(), &mut result);
        if status == errSecItemNotFound {
            return Ok(None);
        }
        if status != 0 || result.is_null() {
            return Err(SecretReadFailure::Unavailable);
        }
        if CFGetTypeID(result) != CFData::type_id() {
            CFRelease(result);
            return Err(SecretReadFailure::Invalid);
        }
        let data = CFData::wrap_under_create_rule(result.cast());
        Ok(Some(data.bytes().to_vec()))
    }
}
/// The existing agent-credential read keeps its search-list semantics.
pub(crate) fn read(service: &str, account: &str) -> Result<Option<Vec<u8>>, SecretReadFailure> {
    read_query(query(service, account))
}
pub(super) fn default_keychain() -> Result<SecKeychain, SecretUnavailable> {
    SecKeychain::default_for_domain(SecPreferencesDomain::User).map_err(|_| SecretUnavailable)
}
fn unlocked(keychain: &SecKeychain) -> Result<(), SecretUnavailable> {
    let mut status = 0;
    // kSecUnlockStateStatus=1, from Security/SecKeychain.h. No unlock request.
    if unsafe { SecKeychainGetStatus(keychain.as_concrete_TypeRef(), &mut status) } != 0
        || status & 1 == 0
    {
        return Err(SecretUnavailable);
    }
    Ok(())
}
fn scoped_query(keychain: &SecKeychain, service: &str, account: &str) -> Query {
    let mut query = query(service, account);
    let keys = CFArray::from_CFTypes(std::slice::from_ref(keychain));
    unsafe {
        query.add(&kSecMatchSearchList.to_void(), &keys.to_void());
    }
    query
}
pub(super) fn read_in(
    keychain: &SecKeychain,
    service: &str,
    account: &str,
) -> Result<Option<Vec<u8>>, SecretUnavailable> {
    unlocked(keychain)?;
    read_query(scoped_query(keychain, service, account)).map_err(|_| SecretUnavailable)
}
pub(super) fn write_in(
    keychain: &SecKeychain,
    service: &str,
    account: &str,
    bytes: &[u8],
) -> Result<(), SecretUnavailable> {
    unlocked(keychain)?;
    let query = scoped_query(keychain, service, account);
    let data = CFData::from_buffer(bytes);
    let mut attributes = Query::new();
    unsafe {
        attributes.add(&kSecValueData.to_void(), &data.to_void());
    }
    let status = unsafe {
        SecItemUpdate(
            query.as_concrete_TypeRef(),
            attributes.as_concrete_TypeRef(),
        )
    };
    if status == 0 {
        return Ok(());
    }
    if status != errSecItemNotFound {
        return Err(SecretUnavailable);
    }
    let mut insertion = self::query(service, account);
    unsafe {
        insertion.add(&kSecUseKeychain.to_void(), &keychain.to_void());
        insertion.add(&kSecValueData.to_void(), &data.to_void());
        if SecItemAdd(insertion.as_concrete_TypeRef(), ptr::null_mut()) != 0 {
            return Err(SecretUnavailable);
        }
    }
    Ok(())
}
pub(super) fn delete_in(
    keychain: &SecKeychain,
    service: &str,
    account: &str,
) -> Result<(), SecretUnavailable> {
    unlocked(keychain)?;
    let query = scoped_query(keychain, service, account);
    let status = unsafe { SecItemDelete(query.as_concrete_TypeRef()) };
    if status == 0 || status == errSecItemNotFound {
        Ok(())
    } else {
        Err(SecretUnavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use security_framework::os::macos::keychain::CreateOptions;
    #[test]
    #[ignore = "creates, locks and deletes only a disposable keychain; never changes the user default"]
    fn locked_disposable_keychain_refuses_all_operations_without_ui() {
        let root = tempfile::tempdir().unwrap();
        let keychain = CreateOptions::new()
            .password("disposable-only")
            .prompt_user(false)
            .create(root.path().join("verification.keychain-db"))
            .unwrap();
        struct Cleanup<'a>(&'a SecKeychain);
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                unsafe {
                    SecKeychainDelete(self.0.as_concrete_TypeRef());
                }
            }
        }
        let cleanup = Cleanup(&keychain);
        write_in(&keychain, "nessa-disposable", "verification", b"test-only").unwrap();
        assert_eq!(
            read_in(&keychain, "nessa-disposable", "verification")
                .unwrap()
                .unwrap(),
            b"test-only"
        );
        assert_eq!(
            unsafe { SecKeychainLock(keychain.as_concrete_TypeRef()) },
            0
        );
        let before = std::time::Instant::now();
        assert_eq!(
            read_in(&keychain, "nessa-disposable", "verification"),
            Err(SecretUnavailable)
        );
        assert_eq!(
            write_in(&keychain, "nessa-disposable", "verification", b"refused"),
            Err(SecretUnavailable)
        );
        assert_eq!(
            delete_in(&keychain, "nessa-disposable", "verification"),
            Err(SecretUnavailable)
        );
        assert!(before.elapsed() < std::time::Duration::from_secs(10));
        drop(cleanup);
    }
}
