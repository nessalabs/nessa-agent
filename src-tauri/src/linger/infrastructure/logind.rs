//! The system-bus logind adapter.
//!
//! This file is the only place that speaks `org.freedesktop.login1`. For the
//! caller's own uid, logind authorizes `org.freedesktop.login1.set-self-linger`,
//! whose stock default since systemd v249 is allow without an administrator.
//! `set-user-linger` is the administrator action, and only for a different uid.
//! Unit tests do not open the bus. `linger_live` does, and only for the
//! throwaway account `lt`. A failure here is a typed zbus or freedesktop error,
//! never a `Display` string.

use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use zbus::blocking::{connection::Builder as ConnectionBuilder, Connection, Proxy};
use zbus::zvariant::OwnedObjectPath;
use zbus::DBusError;

use crate::linger::application::LogindLinger;
use crate::linger::domain::{LingerCall, LingerObservation, LingerSnapshot, LoginUserId};

use super::errors::{classify_call_name, classify_read_name, ENABLE_TIMEOUT, READ_TIMEOUT};

const DESTINATION: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const MANAGER_INTERFACE: &str = "org.freedesktop.login1.Manager";
const USER_INTERFACE: &str = "org.freedesktop.login1.User";
const NO_SUCH_USER: &str = "org.freedesktop.login1.NoSuchUser";
const LINGER_DIR: &str = "/var/lib/systemd/linger";

pub(crate) struct SystemLogind;

impl LogindLinger for SystemLogind {
    fn read(&self) -> LingerSnapshot {
        let user = process_user();
        let Ok(connection) = connect(READ_TIMEOUT) else {
            return LingerSnapshot::new(user, LingerObservation::Unsupported);
        };
        let observation = match linger_property(&connection, user.get()) {
            Ok(true) => LingerObservation::Enabled,
            Ok(false) => LingerObservation::Disabled,
            Err(error) if is_no_such_user(&error) => linger_file_for_user(user.get()),
            Err(error) => observation_from_zbus(&error),
        };
        LingerSnapshot::new(user, observation)
    }

    fn enable(&self, user: LoginUserId) -> LingerCall {
        let Ok(connection) = connect(ENABLE_TIMEOUT) else {
            return LingerCall::Failed;
        };
        let Ok(manager) = proxy(&connection, MANAGER_PATH, MANAGER_INTERFACE) else {
            return LingerCall::Failed;
        };
        // Enable and interactive. There is no argument that turns linger off.
        match manager.call::<_, _, ()>("SetUserLinger", &(user.get(), true, true)) {
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

/// `GetUser` has no user object yet. The linger file is the same bit logind writes.
fn linger_file_for_user(uid: u32) -> LingerObservation {
    let Some(name) = account_name(uid) else {
        return LingerObservation::Unreadable;
    };
    let Some(path) = linger_path(LINGER_DIR, &name) else {
        return LingerObservation::Unreadable;
    };
    linger_record(&path)
}

fn linger_path(dir: &str, name: &str) -> Option<PathBuf> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return None;
    }
    Some(Path::new(dir).join(name))
}

pub(crate) fn linger_record(path: &Path) -> LingerObservation {
    match std::fs::metadata(path) {
        Ok(_) => LingerObservation::Enabled,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => LingerObservation::Disabled,
        Err(_) => LingerObservation::Unreadable,
    }
}

fn account_name(uid: u32) -> Option<String> {
    let mut pwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut buf = vec![0u8; 4096];
    let mut result = std::ptr::null_mut();
    // SAFETY: `pwd` and `result` are writable out-params, and `buf` is writable
    // for `buf.len()` bytes. `getpwuid_r` does not retain those pointers.
    let rc = unsafe {
        libc::getpwuid_r(
            uid,
            pwd.as_mut_ptr(),
            buf.as_mut_ptr().cast(),
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        return None;
    }
    // SAFETY: a non-null result points at `pwd`, and `pw_name` points into `buf`
    // for the rest of this function. The name is copied before either is dropped.
    let name = unsafe { (*result).pw_name };
    if name.is_null() {
        return None;
    }
    let name = unsafe { CStr::from_ptr(name) };
    name.to_str().ok().map(str::to_string)
}

fn call_from_zbus(error: &zbus::Error) -> LingerCall {
    match error {
        zbus::Error::InputOutput(io) if io.kind() == std::io::ErrorKind::TimedOut => {
            LingerCall::Refused
        }
        zbus::Error::InputOutput(_) => LingerCall::Failed,
        zbus::Error::MethodError(name, _, _) => classify_call_name(name.as_str()),
        zbus::Error::FDO(fdo) => match fdo.as_ref() {
            zbus::fdo::Error::ZBus(inner) => call_from_zbus(inner),
            other => classify_call_name(other.name().as_str()),
        },
        _ => LingerCall::Failed,
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

fn is_no_such_user(error: &zbus::Error) -> bool {
    match error {
        zbus::Error::MethodError(name, _, _) => name.as_str() == NO_SUCH_USER,
        zbus::Error::FDO(fdo) => match fdo.as_ref() {
            zbus::fdo::Error::ZBus(inner) => is_no_such_user(inner),
            other => other.name().as_str() == NO_SUCH_USER,
        },
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{linger_path, linger_record, SystemLogind};
    use crate::linger::application::LogindLinger;
    use crate::linger::domain::{LingerCall, LingerObservation};

    #[test]
    fn a_missing_linger_file_is_off_and_a_present_one_is_on() {
        let dir = std::env::temp_dir().join(format!("nessa-linger-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("absent");
        let _ = std::fs::remove_file(&missing);
        assert_eq!(linger_record(&missing), LingerObservation::Disabled);
        let present = dir.join("present");
        std::fs::write(&present, b"").unwrap();
        assert_eq!(linger_record(&present), LingerObservation::Enabled);
        assert!(linger_path("/var/lib/systemd/linger", "lt").is_some());
        assert!(linger_path("/var/lib/systemd/linger", "../lt").is_none());
        assert!(linger_path("/var/lib/systemd/linger", "a/b").is_none());
    }

    /// The real adapter, for the throwaway user `lt` only.
    /// `scripts/desktop/check-linux-linger.sh` runs this inside that user's
    /// logind session. It does not run in an ordinary `cargo test`.
    #[test]
    #[ignore]
    fn linger_live() {
        if std::env::var("NESSA_LINGER_ACCEPTANCE").ok().as_deref() != Some("1") {
            eprintln!("NESSA_LINGER_ACCEPTANCE is not 1; not touching linger");
            return;
        }
        let logind = SystemLogind;
        let before = logind.read();
        let name = super::account_name(before.user().get()).expect("account name");
        assert_eq!(
            name, "lt",
            "linger_live only enables linger for the throwaway user"
        );
        assert_eq!(before.observation(), LingerObservation::Disabled);
        assert_eq!(logind.enable(before.user()), LingerCall::Succeeded);
        assert_eq!(logind.read().observation(), LingerObservation::Enabled);
        assert!(std::path::Path::new("/var/lib/systemd/linger/lt").exists());
    }
}
