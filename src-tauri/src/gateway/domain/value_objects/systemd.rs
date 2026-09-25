//! Validated systemd identities carried by the portable lifecycle journal.
//!
//! These values describe native facts without reading D-Bus or the filesystem.
//! Infrastructure parses outside representations and constructs them; the
//! journal uses the same constructors when restoring persisted records.

use super::ReconciliationTarget;
use std::{error::Error, fmt};

/// Canonical, non-template systemd service unit owned by this gateway.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemdUnitName(String);

impl SystemdUnitName {
    pub fn parse(value: String) -> Result<Self, SystemdEvidenceError> {
        let stem = value
            .strip_suffix(".service")
            .ok_or(SystemdEvidenceError::InvalidUnit)?;
        if stem.is_empty()
            || stem.contains('@')
            || !stem.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'.' | b'-')
            })
        {
            return Err(SystemdEvidenceError::InvalidUnit);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One exact systemd user-manager process behind the well-known bus name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemdManagerIdentity {
    unique_name: String,
    process_id: u32,
    user_id: u32,
}

impl SystemdManagerIdentity {
    pub fn new(
        unique_name: String,
        process_id: u32,
        user_id: u32,
    ) -> Result<Self, SystemdEvidenceError> {
        if !valid_unique_bus_name(&unique_name) || process_id == 0 {
            return Err(SystemdEvidenceError::InvalidManager);
        }
        Ok(Self {
            unique_name,
            process_id,
            user_id,
        })
    }

    pub fn unique_name(&self) -> &str {
        &self.unique_name
    }

    pub fn process_id(&self) -> u32 {
        self.process_id
    }

    pub fn user_id(&self) -> u32 {
        self.user_id
    }
}

/// systemd's 16-byte invocation identity for one service process incarnation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemdInvocationId([u8; 16]);

impl SystemdInvocationId {
    pub fn new(bytes: Vec<u8>) -> Result<Self, SystemdEvidenceError> {
        let bytes: [u8; 16] = bytes
            .try_into()
            .map_err(|_| SystemdEvidenceError::InvalidInvocation)?;
        if bytes == [0; 16] {
            return Err(SystemdEvidenceError::InvalidInvocation);
        }
        Ok(Self(bytes))
    }

    pub fn bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

/// Native manager operation whose returned job is correlated separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemdJobOperation {
    Start,
    Stop,
}

/// Fixed conflict policy for this bounded adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemdJobMode {
    Fail,
}

impl SystemdJobMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fail => "fail",
        }
    }
}

/// Identity returned by `StartUnit` or `StopUnit` before terminal observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemdJobAttempt {
    manager: SystemdManagerIdentity,
    operation: SystemdJobOperation,
    mode: SystemdJobMode,
    unit: SystemdUnitName,
    object_path: String,
    job_id: u32,
}

impl SystemdJobAttempt {
    pub fn new(
        manager: SystemdManagerIdentity,
        operation: SystemdJobOperation,
        mode: SystemdJobMode,
        unit: SystemdUnitName,
        object_path: String,
        job_id: u32,
    ) -> Result<Self, SystemdEvidenceError> {
        let derived = object_path
            .strip_prefix("/org/freedesktop/systemd1/job/")
            .and_then(|value| value.parse::<u32>().ok());
        if job_id == 0 || derived != Some(job_id) {
            return Err(SystemdEvidenceError::InvalidJob);
        }
        Ok(Self {
            manager,
            operation,
            mode,
            unit,
            object_path,
            job_id,
        })
    }

    pub fn manager(&self) -> &SystemdManagerIdentity {
        &self.manager
    }

    pub fn operation(&self) -> SystemdJobOperation {
        self.operation
    }

    pub fn mode(&self) -> SystemdJobMode {
        self.mode
    }

    pub fn unit(&self) -> &SystemdUnitName {
        &self.unit
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn object_path(&self) -> &str {
        &self.object_path
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn job_id(&self) -> u32 {
        self.job_id
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn agrees_with_terminal(
        &self,
        manager: &SystemdManagerIdentity,
        object_path: &str,
        job_id: u32,
        unit: &SystemdUnitName,
    ) -> bool {
        self.manager == *manager
            && self.object_path == object_path
            && self.job_id == job_id
            && self.unit == *unit
    }
}

/// Exact native state that may corroborate one portable incarnation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemdRuntimeObservation {
    target: ReconciliationTarget,
    manager: SystemdManagerIdentity,
    unit: SystemdUnitName,
    invocation: SystemdInvocationId,
    main_process_id: u32,
    enabled: bool,
}

impl SystemdRuntimeObservation {
    pub fn new(
        target: ReconciliationTarget,
        manager: SystemdManagerIdentity,
        unit: SystemdUnitName,
        invocation: SystemdInvocationId,
        main_process_id: u32,
        enabled: bool,
    ) -> Result<Self, SystemdEvidenceError> {
        if target.service() != unit.as_str() || main_process_id == 0 || !enabled {
            return Err(SystemdEvidenceError::ContradictoryRuntime);
        }
        Ok(Self {
            target,
            manager,
            unit,
            invocation,
            main_process_id,
            enabled,
        })
    }

    pub fn target(&self) -> &ReconciliationTarget {
        &self.target
    }

    pub fn manager(&self) -> &SystemdManagerIdentity {
        &self.manager
    }

    pub fn unit(&self) -> &SystemdUnitName {
        &self.unit
    }

    pub fn invocation(&self) -> SystemdInvocationId {
        self.invocation
    }

    pub fn main_process_id(&self) -> u32 {
        self.main_process_id
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemdEvidenceError {
    InvalidUnit,
    InvalidManager,
    InvalidInvocation,
    InvalidJob,
    ContradictoryRuntime,
}

impl fmt::Display for SystemdEvidenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidUnit => "systemd unit name is not canonical",
            Self::InvalidManager => "systemd manager identity is invalid",
            Self::InvalidInvocation => {
                "systemd invocation identity must contain 16 bytes and cannot be all zero"
            }
            Self::InvalidJob => "systemd job path and numeric identity disagree",
            Self::ContradictoryRuntime => {
                "systemd runtime facts disagree with the portable gateway target"
            }
        })
    }
}

impl Error for SystemdEvidenceError {}

fn valid_unique_bus_name(value: &str) -> bool {
    let Some(rest) = value.strip_prefix(':') else {
        return false;
    };
    !rest.is_empty()
        && rest.split('.').count() >= 2
        && rest.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager() -> SystemdManagerIdentity {
        SystemdManagerIdentity::new(":1.42".into(), 100, 501).expect("manager")
    }

    #[test]
    fn unit_and_manager_construction_reject_ambiguous_identity() {
        assert!(SystemdUnitName::parse("nessa-prod.service".into()).is_ok());
        assert!(SystemdUnitName::parse("nessa@prod.service".into()).is_err());
        assert!(SystemdUnitName::parse("../nessa.service".into()).is_err());
        assert!(SystemdManagerIdentity::new("org.freedesktop.systemd1".into(), 1, 501).is_err());
        assert!(SystemdManagerIdentity::new(":1.42".into(), 0, 501).is_err());
    }

    #[test]
    fn returned_job_path_must_derive_the_same_numeric_id() {
        let unit = SystemdUnitName::parse("nessa-prod.service".into()).expect("unit");
        assert!(SystemdJobAttempt::new(
            manager(),
            SystemdJobOperation::Start,
            SystemdJobMode::Fail,
            unit.clone(),
            "/org/freedesktop/systemd1/job/7".into(),
            7,
        )
        .is_ok());
        assert!(SystemdJobAttempt::new(
            manager(),
            SystemdJobOperation::Start,
            SystemdJobMode::Fail,
            unit,
            "/org/freedesktop/systemd1/job/8".into(),
            7,
        )
        .is_err());
    }

    #[test]
    fn native_runtime_requires_portable_unit_pid_and_enablement_agreement() {
        let unit = SystemdUnitName::parse("nessa-prod.service".into()).expect("unit");
        let target =
            ReconciliationTarget::new(unit.as_str().into(), "a".repeat(64), "b".repeat(64))
                .expect("target");
        let invocation = SystemdInvocationId::new(vec![1; 16]).expect("invocation");
        assert!(SystemdRuntimeObservation::new(
            target.clone(),
            manager(),
            unit.clone(),
            invocation,
            77,
            true,
        )
        .is_ok());
        assert!(
            SystemdRuntimeObservation::new(target, manager(), unit, invocation, 0, true,).is_err()
        );
    }
}
