//! Identity of the immutable provider configuration admitted for a workspace.
use super::Failure;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvironmentIdentity {
    id: String,
    name: String,
    version_id: String,
    version: u32,
}
impl EnvironmentIdentity {
    /// Whether Nessa makes VMs in the environment named `name`: only in its
    /// own. Owned here, so a check before anything is made and the identity
    /// a VM's receipt carries hold the same rule.
    pub fn admits(name: &str) -> bool {
        name == "nessa"
    }
    pub fn new(
        id: String,
        name: String,
        version_id: String,
        version: u32,
    ) -> Result<Self, Failure> {
        let uuid =
            |s: &str| uuid::Uuid::parse_str(s).is_ok_and(|id| id.to_string() == s && !id.is_nil());
        if !uuid(&id) || !uuid(&version_id) || !Self::admits(&name) || version == 0 {
            return Err(Failure::EnvironmentUnavailable);
        }
        Ok(Self {
            id,
            name,
            version_id,
            version,
        })
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn version_id(&self) -> &str {
        &self.version_id
    }
    pub fn version(&self) -> u32 {
        self.version
    }
}

/// What a VM was admitted under, on whichever cloud it runs: the identity
/// its cloud proved, held to on every later opening.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admitted {
    /// A Boat VM: the environment and version Boat reported.
    Boat(EnvironmentIdentity),
    /// An Agent37 VM: the template release and image digest it runs.
    Agent37(super::TemplateRelease),
}
impl Admitted {
    /// The environment an existing VM's spec names: Boat's environment,
    /// or Agent37's pinned template.
    pub fn environment(&self) -> &str {
        match self {
            Self::Boat(identity) => identity.name(),
            Self::Agent37(release) => release.template(),
        }
    }
    /// The cloud it runs on.
    pub fn cloud(&self) -> super::Cloud {
        match self {
            Self::Boat(_) => super::Cloud::Boat,
            Self::Agent37(_) => super::Cloud::Agent37,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedEnvironment {
    pub name: Option<String>,
    pub version: Option<u32>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionRefusal {
    MissingEnvironment,
    WrongEnvironment,
    MissingVersion,
    WrongVersion,
    EnvironmentChanged,
    EnvironmentUnavailable,
    InvalidReceipt,
    ReceiptUnavailable,
    LifetimeNotEnforced,
    /// Boat still has to wipe credentials the VM's image carried.
    WipePending,
}
impl AdmissionRefusal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MissingEnvironment => "missing_environment",
            Self::WrongEnvironment => "wrong_environment",
            Self::MissingVersion => "missing_version",
            Self::WrongVersion => "wrong_version",
            Self::EnvironmentChanged => "environment_changed",
            Self::EnvironmentUnavailable => "environment_unavailable",
            Self::InvalidReceipt => "invalid_receipt",
            Self::ReceiptUnavailable => "receipt_unavailable",
            Self::LifetimeNotEnforced => "lifetime_not_enforced",
            Self::WipePending => "wipe_pending",
        }
    }
}

/// Bounded refusal evidence; provider-controlled bodies and credentials never enter it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmissionRejection {
    cause: AdmissionRefusal,
    observed: Option<ObservedEnvironment>,
}
impl AdmissionRejection {
    pub fn new(cause: AdmissionRefusal, observed: Option<ObservedEnvironment>) -> Self {
        if observed
            .as_ref()
            .and_then(|o| o.name.as_ref())
            .is_some_and(|n| n.len() > 128 || !n.is_ascii() || n.chars().any(char::is_control))
        {
            return Self {
                cause: AdmissionRefusal::InvalidReceipt,
                observed: None,
            };
        }
        Self { cause, observed }
    }
    pub fn cause(&self) -> AdmissionRefusal {
        self.cause
    }
    pub fn observed(&self) -> Option<&ObservedEnvironment> {
        self.observed.as_ref()
    }
}

#[cfg(test)]
#[path = "../../../tests/workspaces/environment.rs"]
mod tests;
