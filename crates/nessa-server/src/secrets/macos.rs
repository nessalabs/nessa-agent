//! A noninteractive read from the macOS login keychain.
//!
//! `kSecUseAuthenticationUIFail` is attached to this query only. Security then
//! returns an error when reading would need UI, which lets a background gateway
//! report the source as unavailable without prompting or changing global
//! keychain interaction state.

use std::ptr;

use core_foundation::{
    base::{TCFType, ToVoid},
    boolean::CFBoolean,
    data::CFData,
    dictionary::CFMutableDictionary,
    string::CFString,
};
use core_foundation_sys::base::{CFGetTypeID, CFRelease, CFTypeRef};
use core_foundation_sys::string::CFStringRef;
use security_framework_sys::{
    base::errSecItemNotFound,
    item::{
        kSecAttrAccount, kSecAttrService, kSecClass, kSecClassGenericPassword, kSecReturnData,
        kSecUseAuthenticationUI,
    },
    keychain_item::SecItemCopyMatching,
};

#[derive(Debug)]
pub(crate) enum SecretReadFailure {
    Unavailable,
    Invalid,
}

#[link(name = "Security", kind = "framework")]
extern "C" {
    static kSecUseAuthenticationUIFail: CFStringRef;
}

/// Read one exact generic-password item without allowing authentication UI.
pub(crate) fn read(service: &str, account: &str) -> Result<Option<Vec<u8>>, SecretReadFailure> {
    let service = CFString::new(service);
    let account = CFString::new(account);
    let mut query = CFMutableDictionary::new();
    unsafe {
        query.add(&kSecClass.to_void(), &kSecClassGenericPassword.to_void());
        query.add(&kSecAttrService.to_void(), &service.to_void());
        query.add(&kSecAttrAccount.to_void(), &account.to_void());
        query.add(
            &kSecReturnData.to_void(),
            &CFBoolean::true_value().to_void(),
        );
        query.add(
            &kSecUseAuthenticationUI.to_void(),
            &kSecUseAuthenticationUIFail.to_void(),
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
