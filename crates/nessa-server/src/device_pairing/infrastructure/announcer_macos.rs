//! The local-network announcement on macOS, made by the system's Bonjour
//! responder (`mDNSResponder`) through `dns_sd`.
//!
//! The gateway runs as a background launch agent, and macOS's Local Network
//! privacy silently drops multicast a process sends on its own sockets unless
//! that exact signed binary was allowed; a background process cannot ask. A
//! registration handed to the system responder is sent by the responder, so
//! it reaches the network with no permission and no prompt.
//!
//! The service carries the same type and TXT as [`super::MulticastAnnouncer`]'s.
//! The name is the computer's own (the responder picks it, and renames on a
//! clash), and the host is the machine's `.local` name, which the responder
//! owns. A listener on an unspecified address is announced on every
//! interface; one named address is announced on its interface only. The
//! registration ends when its reference is deallocated.
use super::announcer::{GATEWAY_KEY, LINKING_SERVICE};
use crate::device_pairing::application::{
    Announced, AnnouncementUnavailable, LinkingAnnouncement, LinkingAnnouncer,
};
use std::ffi::{c_char, c_void, CString};
use std::net::IpAddr;
use std::ptr;

type DnsServiceRef = *mut c_void;

/// `kDNSServiceInterfaceIndexAny`.
const ANY_INTERFACE: u32 = 0;

extern "C" {
    // libSystem's dns_sd.h; linked into every macOS binary.
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

/// Announces through the system's Bonjour responder.
#[derive(Default)]
pub struct SystemAnnouncer;

impl LinkingAnnouncer for SystemAnnouncer {
    fn announce(
        &self,
        announcement: &LinkingAnnouncement,
    ) -> Result<Box<dyn Announced>, AnnouncementUnavailable> {
        let ip = announcement.address.ip();
        let interface = if ip.is_unspecified() {
            ANY_INTERFACE
        } else {
            interface_of(ip).ok_or_else(|| {
                AnnouncementUnavailable(format!("no network interface has the address {ip}"))
            })?
        };
        let txt = txt_record(&announcement.gateway)?;
        // dns_sd takes the type without the trailing `local.` domain.
        let regtype = CString::new(
            LINKING_SERVICE
                .trim_end_matches("local.")
                .trim_end_matches('.'),
        )
        .map_err(|error| AnnouncementUnavailable(error.to_string()))?;
        let mut reference: DnsServiceRef = ptr::null_mut();
        // SAFETY: every pointer is valid for the call; a null name, domain,
        // host and callback are documented defaults (computer name, default
        // domain, this machine, no notification). The responder copies what
        // it keeps.
        let status = unsafe {
            DNSServiceRegister(
                &mut reference,
                0,
                interface,
                ptr::null(),
                regtype.as_ptr(),
                ptr::null(),
                ptr::null(),
                announcement.address.port().to_be(),
                txt.len() as u16,
                txt.as_ptr().cast(),
                ptr::null(),
                ptr::null_mut(),
            )
        };
        if status != 0 || reference.is_null() {
            return Err(AnnouncementUnavailable(format!(
                "the system Bonjour responder refused the registration ({status})"
            )));
        }
        Ok(Box::new(Registration(reference)))
    }
}

/// One TXT string, `gw=<id>`, length-prefixed as DNS-SD requires.
fn txt_record(gateway: &str) -> Result<Vec<u8>, AnnouncementUnavailable> {
    let entry = format!("{GATEWAY_KEY}={gateway}");
    let length = u8::try_from(entry.len())
        .map_err(|_| AnnouncementUnavailable("the gateway id is too long for a TXT".into()))?;
    let mut record = Vec::with_capacity(entry.len() + 1);
    record.push(length);
    record.extend_from_slice(entry.as_bytes());
    Ok(record)
}

/// The index of the interface that holds `ip`.
fn interface_of(ip: IpAddr) -> Option<u32> {
    let mut list: *mut libc::ifaddrs = ptr::null_mut();
    // SAFETY: getifaddrs fills `list`, released by freeifaddrs below.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return None;
    }
    let mut found = None;
    let mut entry = list;
    while !entry.is_null() {
        // SAFETY: `entry` is a node of the list getifaddrs returned.
        let current = unsafe { &*entry };
        entry = current.ifa_next;
        if current.ifa_addr.is_null() || address(current.ifa_addr) != Some(ip) {
            continue;
        }
        // SAFETY: ifa_name is a NUL-terminated interface name.
        let index = unsafe { libc::if_nametoindex(current.ifa_name) };
        if index != 0 {
            found = Some(index);
            break;
        }
    }
    // SAFETY: `list` came from getifaddrs and is freed once.
    unsafe { libc::freeifaddrs(list) };
    found
}

fn address(socket: *const libc::sockaddr) -> Option<IpAddr> {
    // SAFETY: `socket` points at a sockaddr whose family says its real type.
    unsafe {
        match i32::from((*socket).sa_family) {
            libc::AF_INET => {
                let v4 = &*(socket as *const libc::sockaddr_in);
                Some(IpAddr::from(u32::from_be(v4.sin_addr.s_addr).to_be_bytes()))
            }
            libc::AF_INET6 => {
                let v6 = &*(socket as *const libc::sockaddr_in6);
                Some(IpAddr::from(v6.sin6_addr.s6_addr))
            }
            _ => None,
        }
    }
}

/// A live registration; deallocating its reference withdraws it.
struct Registration(DnsServiceRef);

// SAFETY: the reference is only ever deallocated, once, by its owner; dns_sd
// allows that from any thread when no other call is using it.
unsafe impl Send for Registration {}
unsafe impl Sync for Registration {}

impl Announced for Registration {
    fn withdraw(self: Box<Self>) {}
}

impl Drop for Registration {
    fn drop(&mut self) {
        // SAFETY: the reference came from a successful DNSServiceRegister and
        // is deallocated only here.
        unsafe { DNSServiceRefDeallocate(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::{txt_record, SystemAnnouncer};
    use crate::device_pairing::application::{LinkingAnnouncement, LinkingAnnouncer as _};
    use std::process::Command;
    use std::time::Duration;

    #[test]
    fn the_txt_record_is_one_length_prefixed_string() {
        assert_eq!(txt_record("gw-1").unwrap(), b"\x07gw=gw-1".to_vec());
        assert!(txt_record(&"x".repeat(300)).is_err());
    }

    /// The real responder: the registration resolves through `dns-sd` to its
    /// port and gateway id while it lives, and is gone once withdrawn. It
    /// looks for its own port and id, not a count of browse results: one
    /// service is listed once per interface, and a gateway already announcing
    /// on the network is listed too. It talks to mDNSResponder, so it runs on
    /// request on a Mac: `cargo test -- --ignored`.
    #[test]
    #[ignore = "registers with this Mac's Bonjour responder"]
    fn a_registration_is_browsable_until_withdrawn() {
        let listed = || {
            let mut child = Command::new("dns-sd")
                .args(["-Z", "_nessa-link._tcp", "local."])
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            std::thread::sleep(Duration::from_secs(3));
            child.kill().unwrap();
            let zone = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
            zone.contains(" 47999 ") && zone.contains("\"gw=gateway-under-test\"")
        };
        let announced = SystemAnnouncer
            .announce(&LinkingAnnouncement {
                address: "0.0.0.0:47999".parse().unwrap(),
                gateway: "gateway-under-test".into(),
            })
            .unwrap();
        let during = listed();
        announced.withdraw();
        let after = listed();
        assert_eq!((during, after), (true, false));
    }
}
