//! Local-network announcement of a paired gateway.
//!
//! The payload is the announcement bytes from [`crate::application::announcement_bytes`].
//! The DNS-SD name is not derived from a gateway id, and the TXT record is
//! only the chunked payload (`p0`, `p1`, …). The pinned key, its fingerprint,
//! and the owner are not registered.
use crate::application::{announcement_txt, payload_from_txt};
use std::net::{IpAddr, SocketAddr};

/// The DNS-SD host this announcement names. It is not derived from a gateway id.
pub fn synthetic_host(instance: &str) -> String {
    format!("{instance}.local.")
}

/// The address record for [`synthetic_host`]. Type 1 is A and type 28 is AAAA.
/// `the_synthetic_host_has_an_address_record_for_the_advertised_ip` is the
/// regression. On macOS, `DNSServiceRegisterRecord` publishes this record and
/// withdraw removes it with the service.
pub fn host_address_record(address: IpAddr) -> (u16, Vec<u8>) {
    match address {
        IpAddr::V4(ip) => (1, ip.octets().to_vec()),
        IpAddr::V6(ip) => (28, ip.octets().to_vec()),
    }
}

/// `Some(0)` is the daemon's confirmation of an address record. A missing
/// callback answer, or any other code, is not a registered record.
/// `DNSServiceRegisterRecord` returns `kDNSServiceErr_BadParam` for a null
/// callback, so that answer cannot be skipped.
/// `address_record_callback_accepts_only_a_confirmed_record` is the regression.
#[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
pub fn address_record_confirmed(error: Option<i32>) -> bool {
    matches!(error, Some(0))
}

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
            let host = super::synthetic_host(instance);
            // The record macOS registers explicitly. mdns-sd builds the same
            // record from the address; a wrong length is a broken helper.
            let (rrtype, rdata) = super::host_address_record(address.ip());
            if !matches!((rrtype, rdata.len()), (1, 4) | (28, 16)) {
                return Err("address record does not match the advertised address".into());
            }
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
    type DnsRecordRef = *mut c_void;

    /// `kDNSServiceClass_IN` and `kDNSServiceFlagsUnique` from dns_sd.h.
    const CLASS_IN: u16 = 1;
    const UNIQUE: u32 = 0x20;
    const ADDRESS_TTL_SECONDS: u32 = 120;
    const POLLIN: i16 = 1;
    const EINTR: i32 = 4;

    type RegisterRecordReply =
        unsafe extern "C" fn(DnsServiceRef, DnsRecordRef, u32, i32, *mut c_void);

    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }

    struct RecordReply {
        error: Option<i32>,
    }

    extern "C" {
        fn poll(fds: *mut PollFd, nfds: u32, timeout: i32) -> i32;
        fn __error() -> *mut i32;
    }

    unsafe extern "C" fn address_record_reply(
        _service: DnsServiceRef,
        _record: DnsRecordRef,
        _flags: u32,
        error: i32,
        context: *mut c_void,
    ) {
        if context.is_null() {
            return;
        }
        // SAFETY: `context` is the `RecordReply` kept alive until the
        // connection that owns this callback is deallocated.
        let reply = unsafe { &mut *context.cast::<RecordReply>() };
        reply.error = Some(error);
    }

    extern "C" {
        fn DNSServiceCreateConnection(sd_ref: *mut DnsServiceRef) -> i32;
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
        fn DNSServiceRegisterRecord(
            sd_ref: DnsServiceRef,
            record_ref: *mut DnsRecordRef,
            flags: u32,
            interface_index: u32,
            fullname: *const c_char,
            rrtype: u16,
            rrclass: u16,
            rdlen: u16,
            rdata: *const c_void,
            ttl: u32,
            callback: RegisterRecordReply,
            context: *mut c_void,
        ) -> i32;
        fn DNSServiceRefSockFD(sd_ref: DnsServiceRef) -> i32;
        fn DNSServiceProcessResult(sd_ref: DnsServiceRef) -> i32;
        fn DNSServiceRemoveRecord(
            sd_ref: DnsServiceRef,
            record_ref: DnsRecordRef,
            flags: u32,
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
            let host_name = super::synthetic_host(instance);
            let host = CString::new(host_name.as_str()).map_err(|error| error.to_string())?;
            let (rrtype, rdata) = super::host_address_record(address.ip());
            let rdlen =
                u16::try_from(rdata.len()).map_err(|_| "address record is too long".to_owned())?;
            let regtype = CString::new(
                PEER_SERVICE
                    .trim_end_matches("local.")
                    .trim_end_matches('.'),
            )
            .map_err(|error| error.to_string())?;
            let txt = txt_bytes(&announcement_txt(payload))?;
            let mut connection: DnsServiceRef = ptr::null_mut();
            // SAFETY: the pointer is valid for the call. The responder copies
            // what it keeps.
            let status = unsafe { DNSServiceCreateConnection(&mut connection) };
            if status != 0 || connection.is_null() {
                return Err(format!("Bonjour refused the address connection ({status})"));
            }
            let mut record: DnsRecordRef = ptr::null_mut();
            let mut reply = Box::new(RecordReply { error: None });
            // SAFETY: `connection` is a live connection, and the name and
            // address bytes live until this call returns. `reply` stays
            // allocated until the connection is deallocated. A non-null host
            // on DNSServiceRegister does not create this address record, and
            // a null callback is BadParam.
            let record_status = unsafe {
                DNSServiceRegisterRecord(
                    connection,
                    &mut record,
                    UNIQUE,
                    0,
                    host.as_ptr(),
                    rrtype,
                    CLASS_IN,
                    rdlen,
                    rdata.as_ptr().cast(),
                    ADDRESS_TTL_SECONDS,
                    address_record_reply,
                    reply.as_mut() as *mut RecordReply as *mut c_void,
                )
            };
            if record_status != 0 || record.is_null() {
                unsafe { DNSServiceRefDeallocate(connection) };
                return Err(format!(
                    "Bonjour refused the address record ({record_status})"
                ));
            }
            if let Err(error) = confirm_address_record(connection, reply.as_mut()) {
                unsafe {
                    DNSServiceRemoveRecord(connection, record, 0);
                    DNSServiceRefDeallocate(connection);
                }
                return Err(error);
            }
            let mut service: DnsServiceRef = ptr::null_mut();
            // SAFETY: the pointers are valid for the call. A null domain and
            // callback are the documented defaults.
            let status = unsafe {
                DNSServiceRegister(
                    &mut service,
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
            if status != 0 || service.is_null() {
                unsafe {
                    DNSServiceRemoveRecord(connection, record, 0);
                    DNSServiceRefDeallocate(connection);
                }
                return Err(format!("Bonjour refused the peer announcement ({status})"));
            }
            Ok(Box::new(Registration {
                connection,
                record,
                service,
                reply,
            }))
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

    fn confirm_address_record(
        connection: DnsServiceRef,
        reply: *mut RecordReply,
    ) -> Result<(), String> {
        let fd = unsafe { DNSServiceRefSockFD(connection) };
        if fd < 0 {
            return Err("Bonjour address connection has no socket".into());
        }
        // SAFETY: `reply` is the callback context and is not borrowed while
        // DNSServiceProcessResult invokes the callback.
        let reported = |reply: *mut RecordReply| unsafe { (*reply).error };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while reported(reply).is_none() {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let mut slot = PollFd {
                fd,
                events: POLLIN,
                revents: 0,
            };
            let millis = i32::try_from(remaining.as_millis()).unwrap_or(2_000);
            let polled = unsafe { poll(&mut slot, 1, millis) };
            if polled < 0 {
                let interrupted = unsafe { *__error() } == EINTR;
                if interrupted {
                    continue;
                }
                return Err("Bonjour address connection could not be read".into());
            }
            if polled == 0 {
                break;
            }
            // SAFETY: `reply` is not borrowed while the callback writes it.
            let status = unsafe { DNSServiceProcessResult(connection) };
            if status != 0 {
                return Err(format!(
                    "Bonjour did not finish the address record ({status})"
                ));
            }
        }
        let error = reported(reply);
        if super::address_record_confirmed(error) {
            Ok(())
        } else {
            Err(match error {
                Some(code) => format!("Bonjour refused the address record ({code})"),
                None => "Bonjour did not confirm the address record".to_owned(),
            })
        }
    }

    struct Registration {
        connection: DnsServiceRef,
        record: DnsRecordRef,
        service: DnsServiceRef,
        /// Keeps the callback context alive until the connection is gone.
        reply: Box<RecordReply>,
    }

    impl Registration {
        fn stop(&mut self) {
            // The callback context outlives every ProcessResult on this connection.
            let _kept = self.reply.error;
            let service = std::mem::replace(&mut self.service, ptr::null_mut());
            if !service.is_null() {
                // SAFETY: the reference came from a successful DNSServiceRegister
                // and is deallocated once.
                unsafe { DNSServiceRefDeallocate(service) };
            }
            let record = std::mem::replace(&mut self.record, ptr::null_mut());
            let connection = std::mem::replace(&mut self.connection, ptr::null_mut());
            if connection.is_null() {
                return;
            }
            if !record.is_null() {
                // SAFETY: `record` was registered on `connection` and is removed once.
                unsafe { DNSServiceRemoveRecord(connection, record, 0) };
            }
            // SAFETY: `connection` came from DNSServiceCreateConnection and is
            // deallocated once, after its address record is removed.
            unsafe { DNSServiceRefDeallocate(connection) };
        }
    }

    // SAFETY: the references are deallocated once, from Drop, and dns_sd allows
    // that from any thread when no other call is using them.
    unsafe impl Send for Registration {}
    unsafe impl Sync for Registration {}

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

pub use multicast::platform as platform_lan_announcer;

#[cfg(test)]
mod tests {
    use super::{host_address_record, synthetic_host};
    use std::net::IpAddr;

    #[test]
    fn the_synthetic_host_has_an_address_record_for_the_advertised_ip() {
        assert_eq!(
            synthetic_host("n0123456789abcdef0"),
            "n0123456789abcdef0.local."
        );
        let v4: IpAddr = "192.0.2.10".parse().unwrap();
        assert_eq!(host_address_record(v4), (1, vec![192, 0, 2, 10]));
        let v6: std::net::Ipv6Addr = "2001:db8::1".parse().unwrap();
        assert_eq!(
            host_address_record(IpAddr::V6(v6)),
            (28, v6.octets().to_vec())
        );
    }

    #[test]
    fn address_record_callback_accepts_only_a_confirmed_record() {
        assert!(super::address_record_confirmed(Some(0)));
        assert!(!super::address_record_confirmed(None));
        // kDNSServiceErr_BadParam, the code a null callback gets.
        assert!(!super::address_record_confirmed(Some(-65540)));
    }
}
