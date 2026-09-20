use std::fmt;

/// Identity of the runtime the gateway would launch.
///
/// Two launches share a fingerprint when they would execute the same files with
/// the same model. That is the unit a first-execution scan is paid for: the
/// desktop host stages each runtime under its own directory, so an install or an
/// update changes the executable path and therefore this value, which is exactly
/// when the operating system scans the files again.
///
/// This is a pure comparison of what was configured. It reads nothing from disk
/// and makes no claim that the files exist or are unchanged in place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeFingerprint {
    executable: String,
    entry: String,
    model: String,
}

/// Why a proposed fingerprint does not identify a runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeFingerprintError {
    /// A part was empty, so it names nothing.
    Empty,
    /// A part exceeded the retained length bound.
    TooLong,
}
impl fmt::Display for RuntimeFingerprintError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "runtime fingerprint parts must not be empty",
            Self::TooLong => "runtime fingerprint parts must be at most 4096 bytes",
        })
    }
}
impl std::error::Error for RuntimeFingerprintError {}

const MAX_PART_BYTES: usize = 4096;

impl RuntimeFingerprint {
    /// Identify a runtime by the executable and entry point that would be run
    /// and the model it would be asked for.
    ///
    /// # Errors
    /// Rejects empty parts and parts longer than 4096 bytes; a fingerprint is
    /// retained and compared, so it is bounded like any other stored identity.
    pub fn new(
        executable: &str,
        entry: &str,
        model: &str,
    ) -> Result<Self, RuntimeFingerprintError> {
        for part in [executable, entry, model] {
            if part.is_empty() {
                return Err(RuntimeFingerprintError::Empty);
            }
            if part.len() > MAX_PART_BYTES {
                return Err(RuntimeFingerprintError::TooLong);
            }
        }
        Ok(Self {
            executable: executable.to_owned(),
            entry: entry.to_owned(),
            model: model.to_owned(),
        })
    }
    /// Executable the runtime would launch.
    pub fn executable(&self) -> &str {
        &self.executable
    }
    /// Entry point handed to that executable.
    pub fn entry(&self) -> &str {
        &self.entry
    }
    /// Model the runtime would be asked for.
    pub fn model(&self) -> &str {
        &self.model
    }
}

/// Whether this runtime has completed a warm-up.
///
/// The two states are the before and after of one transition, which is what an
/// audit record of that transition has to carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WarmUpState {
    /// No completed warm-up is recorded for this runtime.
    Cold,
    /// This runtime has been through a full open and close at least once.
    Warmed,
}
impl WarmUpState {
    /// Stable lowercase identifier for audit records and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cold => "cold",
            Self::Warmed => "warmed",
        }
    }
}
impl fmt::Display for WarmUpState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
#[path = "../../../../tests/agent_warm_up/runtime_fingerprint.rs"]
mod tests;
