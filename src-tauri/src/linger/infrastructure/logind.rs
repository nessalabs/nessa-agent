//! The system-bus logind adapter.
//!
//! This file is the only place that speaks `org.freedesktop.login1`. It is not
//! exercised against a live bus: a real `SetUserLinger` would put an
//! administrator prompt on this machine and might enable linger. Name
//! classification and the decision that consumes these results are tested
//! without it. A failure here is a typed zbus or freedesktop error, never a
//! `Display` string.

use std::time::Duration;

use zbus::blocking::{connection::Builder as ConnectionBuilder, Connection, Proxy};
use zbus::zvariant::OwnedObjectPath;
use zbus::DBusError;

use crate::linger::application::LogindLinger;
use crate::linger::domain::{LingerCall, LingerObservation, LingerSnapshot, LoginUserId};

use super::errors::{
    bus_unavailable, classify_call_name, classify_read_name, enable_arguments, ENABLE_TIMEOUT,
    READ_TIMEOUT,
};

const DESTINATION: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const MANAGER_INTERFACE: &str = "org.freedesktop.login1.Manager";
const USER_INTERFACE: &str = "org.freedesktop.login1.User";

pub(crate) struct SystemLogind;

impl LogindLinger for SystemLogind {
    fn read(&self) -> LingerSnapshot {
        let user = process_user();
        let Ok(connection) = connect(READ_TIMEOUT) else {
            return LingerSnapshot::new(user, bus_unavailable());
        };
        match linger_property(&connection, user.get()) {
            Ok(true) => LingerSnapshot::new(user, LingerObservation::Enabled),
            Ok(false) => LingerSnapshot::new(user, LingerObservation::Disabled),
            Err(error) => LingerSnapshot::new(user, observation_from_zbus(&error)),
        }
    }

    fn enable(&self, user: LoginUserId) -> LingerCall {
        let Ok(connection) = connect(ENABLE_TIMEOUT) else {
            return LingerCall::Unavailable;
        };
        let (uid, enable, interactive) = enable_arguments(user.get());
        let Ok(manager) = proxy(&connection, MANAGER_PATH, MANAGER_INTERFACE) else {
            return LingerCall::Unavailable;
        };
        match manager.call::<_, _, ()>("SetUserLinger", &(uid, enable, interactive)) {
            Ok(()) => LingerCall::Succeeded,
            Err(error) => call_from_zbus(&error),
        }
    }
}

fn process_user() -> LoginUserId {
    // SAFETY: `getuid` has no preconditions and cannot fail.
    LoginUserId::new(unsafe { libc::getuid() })
}

fn connect(timeout: Duration) -> Result<Connection, zbus::Error> {
    ConnectionBuilder::system()?.method_timeout(timeout).build()
}

fn proxy<'a>(
    connection: &'a Connection,
    path: &'a str,
    interface: &'a str,
) -> Result<Proxy<'a>, zbus::Error> {
    Proxy::new(connection, DESTINATION, path, interface)
}

fn linger_property(connection: &Connection, uid: u32) -> Result<bool, zbus::Error> {
    let manager = proxy(connection, MANAGER_PATH, MANAGER_INTERFACE)?;
    let path: OwnedObjectPath = manager.call("GetUser", &(uid,))?;
    let user = proxy(connection, path.as_str(), USER_INTERFACE)?;
    user.get_property("Linger")
}

fn call_from_zbus(error: &zbus::Error) -> LingerCall {
    match error {
        zbus::Error::InputOutput(io) if io.kind() == std::io::ErrorKind::TimedOut => {
            LingerCall::TimedOut
        }
        zbus::Error::InputOutput(_) => LingerCall::Unavailable,
        zbus::Error::MethodError(name, _, _) => classify_call_name(name.as_str()),
        zbus::Error::FDO(fdo) => match fdo.as_ref() {
            zbus::fdo::Error::ZBus(inner) => call_from_zbus(inner),
            other => classify_call_name(other.name().as_str()),
        },
        _ => LingerCall::NotAuthorized,
    }
}

fn observation_from_zbus(error: &zbus::Error) -> LingerObservation {
    match error {
        zbus::Error::InputOutput(_) => LingerObservation::Unsupported,
        zbus::Error::MethodError(name, _, _) => classify_read_name(name.as_str()),
        zbus::Error::FDO(fdo) => match fdo.as_ref() {
            zbus::fdo::Error::ZBus(inner) => observation_from_zbus(inner),
            other => classify_read_name(other.name().as_str()),
        },
        _ => LingerObservation::Unreadable,
    }
}
