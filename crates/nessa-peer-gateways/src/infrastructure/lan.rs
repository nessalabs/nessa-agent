//! Local-network announcement of a paired gateway.
//!
//! The payload is the announcement bytes from [`crate::application::announcement_bytes`].
//! The DNS-SD name is not derived from a gateway id, and the TXT record is
//! only the chunked payload (`p0`, `p1`, …). The pinned key, its fingerprint,
//! and the owner are not registered.
use crate::application::{announcement_txt, payload_from_txt};
use std::net::SocketAddr;

/// The DNS-SD type a paired peer browses. It is not the linking type.
pub const PEER_SERVICE: &str = "_nessa-peer._tcp.local.";

/// A live announcement. Dropping it withdraws the registration.
pub trait LanRegistration: Send + Sync {
    fn withdraw(self: Box<Self>);
}

/// Registers one concrete dial address. Loopback and an unspecified address
/// are the caller's to refuse: this registers the address it is given.
pub trait LanAnnouncer: Send + Sync {
    fn announce(
        &self,
        address: SocketAddr,
        payload: &[u8],
        instance: &str,
    ) -> Result<Box<dyn LanRegistration>, String>;
}

/// The properties a registration may carry. Anything else is not this announcement.
pub fn registration_properties(payload: &[u8]) -> Vec<(String, String)> {
    announcement_txt(payload)
}

pub fn payload_from_registration(
    properties: &[(impl AsRef<str>, impl AsRef<str>)],
) -> Option<Vec<u8>> {
    payload_from_txt(properties)
}

#[cfg(not(target_os = "macos"))]
mod multicast {
    use super::{LanAnnouncer, LanRegistration, PEER_SERVICE};
    use crate::application::announcement_txt;
    use mdns_sd::{ServiceDaemon, ServiceInfo};
    use std::net::SocketAddr;

    pub struct MdnsLan;

    impl LanAnnouncer for MdnsLan {
        fn announce(
            &self,
            address: SocketAddr,
            payload: &[u8],
            instance: &str,
        ) -> Result<Box<dyn LanRegistration>, String> {
            let host = format!("{instance}.local.");
            let properties = announcement_txt(payload);
            let borrowed: Vec<(&str, &str)> = properties
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect();
            let info = ServiceInfo::new(
                PEER_SERVICE,
                instance,
                &host,
                address.ip(),
                address.port(),
                &borrowed[..],
            )
            .map_err(|error| error.to_string())?;
            let fullname = info.get_fullname().to_owned();
            let daemon = ServiceDaemon::new().map_err(|error| error.to_string())?;
            if let Err(error) = daemon.register(info) {
                let _ = daemon.shutdown();
                return Err(error.to_string());
            }
            Ok(Box::new(Registration {
                daemon: Some(daemon),
                fullname,
            }))
        }
    }

    struct Registration {
        daemon: Option<ServiceDaemon>,
        fullname: String,
    }

    impl Registration {
        fn stop(&mut self) {
            if let Some(daemon) = self.daemon.take() {
                let _ = daemon.unregister(&self.fullname);
                let _ = daemon.shutdown();
            }
        }
    }

    impl LanRegistration for Registration {
        fn withdraw(mut self: Box<Self>) {
            self.stop();
        }
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            self.stop();
        }
    }

    pub fn platform() -> std::sync::Arc<dyn LanAnnouncer> {
        std::sync::Arc::new(MdnsLan)
    }
}

#[cfg(target_os = "macos")]
mod multicast {
    use super::{LanAnnouncer, LanRegistration, PEER_SERVICE};
    use crate::application::announcement_txt;
    use std::ffi::{c_char, c_void, CString};
    use std::net::SocketAddr;
    use std::ptr;

    type DnsServiceRef = *mut c_void;

    extern "C" {
        fn DNSServiceRegister(
            sd_ref: *mut DnsServiceRef,
            flags: u32,
            interface_index: u32,
            name: *const c_char,
            regtype: *const c_char,
            domain: *const c_char,
            host: *const c_char,
            port: u16,
            txt_len: u16,
            txt_record: *const c_void,
            callback: *const c_void,
            context: *mut c_void,
        ) -> i32;
        fn DNSServiceRefDeallocate(sd_ref: DnsServiceRef);
    }

    pub struct MdnsLan;

    impl LanAnnouncer for MdnsLan {
        fn announce(
            &self,
            address: SocketAddr,
            payload: &[u8],
            instance: &str,
        ) -> Result<Box<dyn LanRegistration>, String> {
            let name = CString::new(instance).map_err(|error| error.to_string())?;
            let host =
                CString::new(format!("{instance}.local.")).map_err(|error| error.to_string())?;
            let regtype = CString::new(
                PEER_SERVICE
                    .trim_end_matches("local.")
                    .trim_end_matches('.'),
            )
            .map_err(|error| error.to_string())?;
            let txt = txt_bytes(&announcement_txt(payload))?;
            let mut reference: DnsServiceRef = ptr::null_mut();
            // SAFETY: the pointers are valid for the call. The responder copies
            // what it keeps. A null domain and callback are the documented defaults.
            let status = unsafe {
                DNSServiceRegister(
                    &mut reference,
                    0,
                    0,
                    name.as_ptr(),
                    regtype.as_ptr(),
                    ptr::null(),
                    host.as_ptr(),
                    address.port().to_be(),
                    u16::try_from(txt.len())
                        .map_err(|_| "announcement TXT is too long".to_owned())?,
                    txt.as_ptr().cast(),
                    ptr::null(),
                    ptr::null_mut(),
                )
            };
            if status != 0 || reference.is_null() {
                return Err(format!("Bonjour refused the peer announcement ({status})"));
            }
            Ok(Box::new(Registration(reference)))
        }
    }

    fn txt_bytes(properties: &[(String, String)]) -> Result<Vec<u8>, String> {
        let mut record = Vec::new();
        for (key, value) in properties {
            let entry = format!("{key}={value}");
            let length =
                u8::try_from(entry.len()).map_err(|_| "a TXT string is too long".to_owned())?;
            record.push(length);
            record.extend_from_slice(entry.as_bytes());
        }
        Ok(record)
    }

    struct Registration(DnsServiceRef);

    // SAFETY: the reference is deallocated once, from Drop, and dns_sd allows
    // that from any thread when no other call is using it.
    unsafe impl Send for Registration {}
    unsafe impl Sync for Registration {}

    impl LanRegistration for Registration {
        fn withdraw(self: Box<Self>) {}
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            // SAFETY: the reference came from a successful DNSServiceRegister.
            unsafe { DNSServiceRefDeallocate(self.0) };
        }
    }

    pub fn platform() -> std::sync::Arc<dyn LanAnnouncer> {
        std::sync::Arc::new(MdnsLan)
    }
}

pub use multicast::platform as platform_lan_announcer;
