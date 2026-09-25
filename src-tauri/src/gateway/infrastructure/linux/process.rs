use crate::gateway::{
    application::{GatewayError, GatewayStopProofToken, GatewayStopSession},
    domain::value_objects::{
        AuditDeliveryReceipt, LifecycleCommandResult, ReconciliationIncarnation,
        SystemdRuntimeObservation,
    },
};
use std::os::fd::{FromRawFd, OwnedFd};

pub(super) struct LinuxProcessHandle {
    pidfd: OwnedFd,
    process_id: u32,
}

pub(super) struct LinuxSignalAuthority {
    token: GatewayStopProofToken,
    pidfd: OwnedFd,
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
        handle: LinuxProcessHandle,
        token: GatewayStopProofToken,
        native: SystemdRuntimeObservation,
        portable: ReconciliationIncarnation,
        observation_version: u64,
    ) -> Result<Self, GatewayError> {
        if native.target() != portable.target()
            || native.main_process_id() != portable.process_id()
            || handle.process_id != portable.process_id()
            || observation_version == 0
        {
            return Err(GatewayError::Stop(
                "Linux signal proof disagrees with the portable gateway incarnation".into(),
            ));
        }
        if !handle.is_live() {
            return Err(GatewayError::Stop(
                "The gateway process exited during exact signal proof".into(),
            ));
        }
        Ok(Self {
            token,
            pidfd: handle.pidfd,
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
            pidfd,
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
        let dispatched = pidfd_send_signal(std::os::fd::AsRawFd::as_raw_fd(&pidfd), signal);
        Ok(if dispatched == 0 {
            LifecycleCommandResult::Accepted
        } else {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                LifecycleCommandResult::Rejected(
                    "The proved gateway process exited before signal dispatch".into(),
                )
            } else {
                LifecycleCommandResult::Failed(error.to_string())
            }
        })
    }
}

impl LinuxProcessHandle {
    pub fn open(process_id: u32) -> Result<Self, GatewayError> {
        let descriptor = pidfd_open(process_id);
        if descriptor < 0 {
            return Err(GatewayError::Stop(format!(
                "The gateway process cannot be held for exact signal delivery: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(Self {
            pidfd: unsafe { OwnedFd::from_raw_fd(descriptor as i32) },
            process_id,
        })
    }

    pub fn is_live(&self) -> bool {
        pidfd_send_signal(std::os::fd::AsRawFd::as_raw_fd(&self.pidfd), 0) == 0
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
