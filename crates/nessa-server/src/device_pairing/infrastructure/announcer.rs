//! The local-network announcement, made with multicast DNS-SD (Bonjour).
//!
//! One responder thread per announcement, started when the native listener
//! starts and shut down when it stops. Only addresses the listener answers on
//! are announced: a listener on `0.0.0.0` names each interface's IPv4
//! addresses as they come and go (never IPv6, which it does not serve, and
//! never loopback), `[::]` names both families, and one named address is
//! announced alone. A failure to start is reported, not fatal: the gateway
//! still pairs by address.
//!
//! The host record is the gateway's own (`nessa-<id>.local.`), never the
//! machine's `.local` name: the operating system's responder owns that one,
//! and a second answer for it with other addresses reads as a name conflict.
use crate::device_pairing::application::{
    Announced, AnnouncementUnavailable, LinkingAnnouncement, LinkingAnnouncer,
};
use mdns_sd::{IfKind, ServiceDaemon, ServiceInfo};
use std::net::IpAddr;

/// The DNS-SD service type a phone browses for. The iOS app lists the same
/// name in its `NSBonjourServices`.
pub const LINKING_SERVICE: &str = "_nessa-link._tcp.local.";
/// The TXT key holding the gateway's public id.
pub const GATEWAY_KEY: &str = "gw";

/// Announces over multicast DNS-SD.
#[derive(Default)]
pub struct MulticastAnnouncer;

impl LinkingAnnouncer for MulticastAnnouncer {
    fn announce(
        &self,
        announcement: &LinkingAnnouncement,
    ) -> Result<Box<dyn Announced>, AnnouncementUnavailable> {
        let unavailable = |error: mdns_sd::Error| AnnouncementUnavailable(error.to_string());
        let name = machine_name().unwrap_or_else(|| "Nessa".to_owned());
        let host = format!("{}.local.", host_label(&announcement.gateway));
        let properties = [(GATEWAY_KEY, announcement.gateway.as_str())];
        let ip = announcement.address.ip();
        let port = announcement.address.port();
        let info = if ip.is_unspecified() {
            ServiceInfo::new(LINKING_SERVICE, &name, &host, (), port, &properties[..])
                .map(ServiceInfo::enable_addr_auto)
        } else {
            ServiceInfo::new(LINKING_SERVICE, &name, &host, ip, port, &properties[..])
        }
        .map_err(unavailable)?;
        let fullname = info.get_fullname().to_owned();
        let daemon = ServiceDaemon::new().map_err(unavailable)?;
        let started = served_interfaces(&daemon, ip).and_then(|()| daemon.register(info));
        if let Err(error) = started {
            let _ = daemon.shutdown();
            return Err(unavailable(error));
        }
        Ok(Box::new(Multicast {
            daemon: Some(daemon),
            fullname,
        }))
    }
}

/// Keep the responder to the interfaces the listener serves on `ip`.
fn served_interfaces(daemon: &ServiceDaemon, ip: IpAddr) -> Result<(), mdns_sd::Error> {
    daemon.disable_interface(vec![IfKind::LoopbackV4, IfKind::LoopbackV6])?;
    match ip {
        IpAddr::V4(any) if any.is_unspecified() => daemon.disable_interface(IfKind::IPv6),
        IpAddr::V6(any) if any.is_unspecified() => Ok(()),
        one => {
            daemon.disable_interface(IfKind::All)?;
            daemon.enable_interface(IfKind::Addr(one))
        }
    }
}

struct Multicast {
    daemon: Option<ServiceDaemon>,
    fullname: String,
}

impl Multicast {
    /// Say goodbye on the network, then stop the responder. Neither waits:
    /// a phone that misses the goodbye drops the entry when its TTL runs out.
    fn stop(&mut self) {
        if let Some(daemon) = self.daemon.take() {
            let _ = daemon.unregister(&self.fullname);
            let _ = daemon.shutdown();
        }
    }
}

impl Announced for Multicast {
    fn withdraw(mut self: Box<Self>) {
        self.stop();
    }
}

impl Drop for Multicast {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The gateway's own DNS host label, `nessa-` and the first 16 letters and
/// digits of its id, lower-cased: no other responder answers for it.
fn host_label(gateway: &str) -> String {
    let id: String = gateway
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .take(16)
        .collect();
    if id.is_empty() {
        "nessa".to_owned()
    } else {
        format!("nessa-{id}")
    }
}

/// This machine's name as the operating system reports it, for what a phone
/// lists. `None` when it cannot be read; callers name the gateway instead.
fn machine_name() -> Option<String> {
    #[cfg(unix)]
    {
        let mut buffer = [0u8; 256];
        // SAFETY: the buffer is valid for its whole length, and gethostname
        // writes at most that many bytes.
        let read = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
        if read != 0 {
            return None;
        }
        let end = buffer
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(buffer.len());
        let name = String::from_utf8_lossy(&buffer[..end]);
        let name = name.trim_end_matches(".local").trim();
        (!name.is_empty()).then(|| name.to_owned())
    }
    #[cfg(not(unix))]
    {
        std::env::var("COMPUTERNAME")
            .ok()
            .filter(|name| !name.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::host_label;

    /// The real responder, browsed from this process: the service resolves
    /// to its port and gateway id. It needs multicast on a network interface
    /// (loopback is never announced), which hosted CI runners do not all
    /// give, so it runs on request: `cargo test -- --ignored`. The stress
    /// check's `linking` step browses for the running gateway the same way.
    #[test]
    #[ignore = "needs multicast on a network interface"]
    fn an_announcement_is_found_by_a_browser_on_this_machine() {
        use super::{
            LinkingAnnouncement, LinkingAnnouncer as _, MulticastAnnouncer, GATEWAY_KEY,
            LINKING_SERVICE,
        };
        use mdns_sd::{ServiceDaemon, ServiceEvent};
        use std::time::{Duration, Instant};
        let announced = MulticastAnnouncer
            .announce(&LinkingAnnouncement {
                address: "0.0.0.0:47999".parse().unwrap(),
                gateway: "gateway-under-test".into(),
            })
            .unwrap();
        let browser = ServiceDaemon::new().unwrap();
        let events = browser.browse(LINKING_SERVICE).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut resolved = None;
        while resolved.is_none() && Instant::now() < deadline {
            if let Ok(ServiceEvent::ServiceResolved(info)) =
                events.recv_timeout(Duration::from_millis(250))
            {
                if info.get_property_val_str(GATEWAY_KEY) == Some("gateway-under-test") {
                    resolved = Some((
                        info.get_port(),
                        info.get_property_val_str(GATEWAY_KEY).map(str::to_owned),
                    ));
                }
            }
        }
        announced.withdraw();
        let _ = browser.shutdown();
        assert_eq!(resolved, Some((47_999, Some("gateway-under-test".into()))));
    }

    #[test]
    fn the_host_label_is_the_gateways_own() {
        assert_eq!(
            host_label("GW_0123-4567:89abCDEF/xyz"),
            "nessa-gw0123456789abcd"
        );
        assert_eq!(host_label("..."), "nessa");
    }
}
