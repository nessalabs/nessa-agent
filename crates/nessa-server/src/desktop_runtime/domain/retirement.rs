use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Fingerprint(String);
impl Fingerprint {
    pub(crate) fn new(value: String) -> Result<Self, &'static str> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
        {
            return Err("invalid runtime fingerprint");
        }
        Ok(Self(value))
    }
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg(any(target_os = "macos", target_os = "linux", test))]
pub(crate) struct RetirementRequest {
    id: String,
    target: Fingerprint,
    running_instance: RuntimeInstance,
    running_generation: ServiceGeneration,
    target_generation: ServiceGeneration,
}
#[cfg(any(target_os = "macos", target_os = "linux", test))]
impl RetirementRequest {
    pub(crate) fn new(
        id: String,
        target: String,
        running_instance: String,
        running_generation: String,
        target_generation: String,
    ) -> Result<Self, &'static str> {
        if running_generation == target_generation {
            return Err("retirement requires a different target service generation");
        }
        let parsed = Uuid::parse_str(&id).map_err(|_| "invalid upgrade request ID")?;
        if parsed.to_string() != id {
            return Err("noncanonical upgrade request ID");
        }
        Ok(Self {
            id,
            target: Fingerprint::new(target)?,
            running_instance: RuntimeInstance::new(running_instance)?,
            running_generation: ServiceGeneration::new(running_generation)?,
            target_generation: ServiceGeneration::new(target_generation)?,
        })
    }
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
    pub(crate) fn running_generation(&self) -> &ServiceGeneration {
        &self.running_generation
    }
    pub(crate) fn target_generation(&self) -> &ServiceGeneration {
        &self.target_generation
    }
    pub(crate) fn running_instance(&self) -> &RuntimeInstance {
        &self.running_instance
    }
    pub(crate) fn target(&self) -> &Fingerprint {
        &self.target
    }
}

#[cfg(test)]
#[path = "../../../tests/desktop_runtime/domain.rs"]
mod tests;

/// A single process incarnation, distinct from its reusable runtime fingerprint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RuntimeInstance(String);
impl RuntimeInstance {
    pub(crate) fn new(value: String) -> Result<Self, &'static str> {
        let parsed = Uuid::parse_str(&value).map_err(|_| "invalid runtime instance")?;
        if parsed.to_string() != value {
            return Err("noncanonical runtime instance");
        }
        Ok(Self(value))
    }
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunningRuntime {
    fingerprint: Fingerprint,
    instance: RuntimeInstance,
    process_id: u32,
    generation: ServiceGeneration,
}
impl RunningRuntime {
    pub(crate) fn new(
        fingerprint: String,
        instance: String,
        process_id: u32,
        generation: String,
    ) -> Result<Self, &'static str> {
        if process_id == 0 {
            return Err("invalid runtime process ID");
        }
        Ok(Self {
            fingerprint: Fingerprint::new(fingerprint)?,
            instance: RuntimeInstance::new(instance)?,
            process_id,
            generation: ServiceGeneration::new(generation)?,
        })
    }
    pub(crate) fn generation(&self) -> &ServiceGeneration {
        &self.generation
    }
    pub(crate) fn fingerprint(&self) -> &Fingerprint {
        &self.fingerprint
    }
    pub(crate) fn instance(&self) -> &RuntimeInstance {
        &self.instance
    }
    pub(crate) fn process_id(&self) -> u32 {
        self.process_id
    }
    #[cfg(any(target_os = "macos", target_os = "linux", test))]
    pub(crate) fn accepts(&self, request: &RetirementRequest) -> bool {
        &self.instance == request.running_instance()
            && &self.generation == request.running_generation()
    }
}
/// Validated admitted retirement evidence.
///
/// Admission remains fenced after restart once a retirement was admitted, even
/// when cleanup or audit acknowledgement failed. Confirmation separately tells
/// the host whether it may replace the process.
#[derive(Clone, Debug)]
#[cfg(any(target_os = "macos", target_os = "linux", test))]
pub(crate) struct RetirementFence {
    request: RetirementRequest,
    running: Fingerprint,
    cause: RetirementCause,
    confirmed: bool,
}
#[cfg(any(target_os = "macos", target_os = "linux", test))]
impl RetirementFence {
    pub(crate) fn new(
        request: RetirementRequest,
        running: &RunningRuntime,
        cause: RetirementCause,
        confirmed: bool,
        has_failure: bool,
    ) -> Result<Self, &'static str> {
        if !running.accepts(&request) {
            return Err(
                "admitted retirement instance or generation differs from the requested runtime",
            );
        }
        if confirmed == has_failure {
            return Err(if confirmed {
                "retirement success contradicts failed cleanup or audit"
            } else {
                "unconfirmed retirement lacks a cleanup or audit failure"
            });
        }
        if confirmed && !cause.is_gateway_upgrade() {
            return Err("confirmed retirement has invalid lifecycle authority");
        }
        Ok(Self {
            request,
            running: running.fingerprint().clone(),
            cause,
            confirmed,
        })
    }
    pub(crate) fn confirmed(&self) -> bool {
        self.confirmed
    }
    #[cfg(test)]
    pub(crate) fn retirement_request_id(&self) -> &str {
        self.cause.request_id()
    }
    pub(crate) fn cause(&self) -> &RetirementCause {
        &self.cause
    }
    pub(crate) fn applies_to(&self, runtime: &RunningRuntime) -> bool {
        &self.running == runtime.fingerprint()
            && self.request.running_generation() == runtime.generation()
    }
}

/// Original lifecycle attribution retained by an admitted retirement fence.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg(any(target_os = "macos", target_os = "linux", test))]
pub(crate) struct RetirementCause {
    principal_id: String,
    surface_id: String,
    request_id: String,
}
#[cfg(any(target_os = "macos", target_os = "linux", test))]
impl RetirementCause {
    const MAX_IDENTITY_BYTES: usize = 256;

    pub(crate) fn new(
        principal_id: String,
        surface_id: String,
        request_id: String,
    ) -> Result<Self, &'static str> {
        for value in [&principal_id, &surface_id, &request_id] {
            if value.trim().is_empty() || value.len() > Self::MAX_IDENTITY_BYTES {
                return Err("invalid retirement cause identity");
            }
        }
        RuntimeInstance::new(request_id.clone())?;
        Ok(Self {
            principal_id,
            surface_id,
            request_id,
        })
    }
    pub(crate) fn principal_id(&self) -> &str {
        &self.principal_id
    }
    pub(crate) fn surface_id(&self) -> &str {
        &self.surface_id
    }
    pub(crate) fn request_id(&self) -> &str {
        &self.request_id
    }
    fn is_gateway_upgrade(&self) -> bool {
        self.principal_id == "gateway" && self.surface_id == "gateway_upgrade"
    }
}

/// Validates the complete result tuple before it crosses the persistence boundary.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
pub(crate) fn validate_retirement_evidence(
    request: RetirementRequest,
    running: &RunningRuntime,
    cause: Option<RetirementCause>,
    retired: bool,
    has_failure: bool,
) -> Result<Option<RetirementFence>, &'static str> {
    if retired && has_failure {
        return Err("retirement success contradicts failed cleanup or audit");
    }
    if !retired && !has_failure {
        return Err("failed retirement lacks a cleanup or audit failure");
    }
    match cause {
        Some(cause) => {
            RetirementFence::new(request, running, cause, retired, has_failure).map(Some)
        }
        None if running.accepts(&request) => {
            Err("admitted retirement lacks original lifecycle attribution")
        }
        None if retired => Err("successful retirement lacks original lifecycle attribution"),
        None => Ok(None),
    }
}

/// Identity of the service definition, independent of the runtime's content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ServiceGeneration(Fingerprint);
impl ServiceGeneration {
    pub(crate) fn new(value: String) -> Result<Self, &'static str> {
        Ok(Self(Fingerprint::new(value)?))
    }
    pub(crate) fn as_str(&self) -> &str {
        self.0.as_str()
    }
}
