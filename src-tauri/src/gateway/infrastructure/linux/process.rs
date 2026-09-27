use crate::gateway::{
    application::{GatewayError, GatewayStopProofToken, GatewayStopSession},
    domain::value_objects::{
        AuditDeliveryReceipt, LifecycleCommandResult, ReconciliationIncarnation,
        SystemdRuntimeObservation,
    },
};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

pub(super) trait LinuxProcess: Send {
    fn process_id(&self) -> u32;
    fn is_live(&self) -> bool;
    fn signal(self: Box<Self>, signal: libc::c_int) -> Result<(), LinuxProcessSignalError>;
}

pub(super) trait LinuxProcessFactory: Send + Sync {
    fn open(&self, process_id: u32) -> Result<Box<dyn LinuxProcess>, String>;
}

pub(super) struct NativeLinuxProcessFactory;

struct PidfdProcess {
    pidfd: OwnedFd,
    process_id: u32,
}

pub(super) enum LinuxProcessSignalError {
    Exited,
    Failed(String),
}

pub(super) struct LinuxSignalAuthority {
    token: GatewayStopProofToken,
    process: Box<dyn LinuxProcess>,
    native: SystemdRuntimeObservation,
    portable: ReconciliationIncarnation,
    observation_version: u64,
}

pub(super) fn verify_pidfd_support() -> Result<(), String> {
    let descriptor = pidfd_open(std::process::id());
    if descriptor < 0 {
        return Err(format!(
            "This Linux host cannot provide pidfd process identity: {}",
            std::io::Error::last_os_error()
        ));
    }
    unsafe { libc::close(descriptor as i32) };
    Ok(())
}

impl LinuxSignalAuthority {
    pub fn corroborate(
        process: Box<dyn LinuxProcess>,
        token: GatewayStopProofToken,
        native: SystemdRuntimeObservation,
        portable: ReconciliationIncarnation,
        observation_version: u64,
    ) -> Result<Self, GatewayError> {
        if native.target() != portable.target()
            || native.main_process_id() != portable.process_id()
            || process.process_id() != portable.process_id()
            || observation_version == 0
        {
            return Err(GatewayError::Stop(
                "Linux signal proof disagrees with the portable gateway incarnation".into(),
            ));
        }
        if !process.is_live() {
            return Err(GatewayError::Stop(
                "The gateway process exited during exact signal proof".into(),
            ));
        }
        Ok(Self {
            token,
            process,
            native,
            portable,
            observation_version,
        })
    }

    pub fn dispatch(
        self,
        session: &GatewayStopSession,
        plan: &AuditDeliveryReceipt,
        signal: libc::c_int,
    ) -> Result<LifecycleCommandResult, GatewayError> {
        let Self {
            token,
            process,
            native,
            portable,
            observation_version,
        } = self;
        if native.target() != portable.target() || native.main_process_id() != portable.process_id()
        {
            return Err(GatewayError::Stop(
                "Linux signal witness changed before application proof".into(),
            ));
        }
        session.prove(&token, portable.clone(), observation_version)?;
        session.claim(token, plan, &portable, observation_version)?;
        // The consume-once claim and syscall are adjacent. The guard already
        // owns the open pidfd; no D-Bus, filesystem, health, or clock read is
        // permitted between these two statements.
        Ok(match process.signal(signal) {
            Ok(()) => LifecycleCommandResult::Accepted,
            Err(LinuxProcessSignalError::Exited) => LifecycleCommandResult::Rejected(
                "The proved gateway process exited before signal dispatch".into(),
            ),
            Err(LinuxProcessSignalError::Failed(error)) => LifecycleCommandResult::Failed(error),
        })
    }
}

impl LinuxProcessFactory for NativeLinuxProcessFactory {
    fn open(&self, process_id: u32) -> Result<Box<dyn LinuxProcess>, String> {
        let descriptor = pidfd_open(process_id);
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(Box::new(PidfdProcess {
            pidfd: unsafe { OwnedFd::from_raw_fd(descriptor as i32) },
            process_id,
        }))
    }
}

impl LinuxProcess for PidfdProcess {
    fn process_id(&self) -> u32 {
        self.process_id
    }

    fn is_live(&self) -> bool {
        pidfd_send_signal(self.pidfd.as_raw_fd(), 0) == 0
    }

    fn signal(self: Box<Self>, signal: libc::c_int) -> Result<(), LinuxProcessSignalError> {
        if pidfd_send_signal(self.pidfd.as_raw_fd(), signal) == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Err(LinuxProcessSignalError::Exited)
        } else {
            Err(LinuxProcessSignalError::Failed(error.to_string()))
        }
    }
}

#[cfg(target_os = "linux")]
fn pidfd_open(process_id: u32) -> libc::c_long {
    unsafe {
        libc::syscall(
            libc::SYS_pidfd_open,
            libc::pid_t::try_from(process_id).unwrap_or(-1),
            0,
        )
    }
}

#[cfg(not(target_os = "linux"))]
fn pidfd_open(_process_id: u32) -> libc::c_long {
    -1
}

#[cfg(target_os = "linux")]
fn pidfd_send_signal(descriptor: i32, signal: libc::c_int) -> libc::c_long {
    unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            descriptor,
            signal,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    }
}

#[cfg(not(target_os = "linux"))]
fn pidfd_send_signal(_descriptor: i32, _signal: libc::c_int) -> libc::c_long {
    -1
}
