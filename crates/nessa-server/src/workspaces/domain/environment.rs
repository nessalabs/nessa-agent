//! Identity of the immutable provider configuration admitted for a workspace.
use super::Failure;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EnvironmentIdentity {
    id: String,
    name: String,
    version_id: String,
    version: u32,
}
impl EnvironmentIdentity {
    pub(crate) fn new(
        id: String,
        name: String,
        version_id: String,
        version: u32,
    ) -> Result<Self, Failure> {
        let uuid =
            |s: &str| uuid::Uuid::parse_str(s).is_ok_and(|id| id.to_string() == s && !id.is_nil());
        if !uuid(&id) || !uuid(&version_id) || name != "nessa" || version == 0 {
            return Err(Failure::EnvironmentUnavailable);
        }
        Ok(Self {
            id,
            name,
            version_id,
            version,
        })
    }
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    pub(crate) fn version_id(&self) -> &str {
        &self.version_id
    }
    pub(crate) fn version(&self) -> u32 {
        self.version
    }
    pub(crate) fn check(&self, observed: &ObservedEnvironment) -> Result<(), AdmissionRefusal> {
        match (&observed.name, observed.version) {
            (None, _) => Err(AdmissionRefusal::MissingEnvironment),
            (Some(name), _) if name != &self.name => Err(AdmissionRefusal::WrongEnvironment),
            (_, None) => Err(AdmissionRefusal::MissingVersion),
            (_, Some(version)) if version != self.version => Err(AdmissionRefusal::WrongVersion),
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ObservedEnvironment {
    pub(crate) name: Option<String>,
    pub(crate) version: Option<u32>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AdmissionRefusal {
    MissingEnvironment,
    WrongEnvironment,
    MissingVersion,
    WrongVersion,
    EnvironmentChanged,
    EnvironmentUnavailable,
    InvalidReceipt,
    ReceiptUnavailable,
}
impl AdmissionRefusal {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::MissingEnvironment => "missing_environment",
            Self::WrongEnvironment => "wrong_environment",
            Self::MissingVersion => "missing_version",
            Self::WrongVersion => "wrong_version",
            Self::EnvironmentChanged => "environment_changed",
            Self::EnvironmentUnavailable => "environment_unavailable",
            Self::InvalidReceipt => "invalid_receipt",
            Self::ReceiptUnavailable => "receipt_unavailable",
        }
    }
}

/// Immutable observed configuration facts; admission owns their one supported posture.
pub(crate) struct EnvironmentConfiguration {
    pub(crate) safe_for_third_parties: bool,
    pub(crate) pass_github: bool,
    pub(crate) pass_secrets: bool,
    pub(crate) pass_sandbox_credentials: bool,
    pub(crate) pass_agents_credentials: bool,
    pub(crate) has_contents: bool,
    pub(crate) has_secret_files: bool,
    pub(crate) has_repositories: bool,
    pub(crate) owner_default_profile: bool,
    pub(crate) proxy_ai_credentials: bool,
}
impl EnvironmentConfiguration {
    pub(crate) fn admit(self) -> Result<(), Failure> {
        if self.safe_for_third_parties
            || self.pass_github
            || self.pass_secrets
            || self.pass_sandbox_credentials
            || !self.pass_agents_credentials
            || self.has_contents
            || self.has_secret_files
            || self.has_repositories
            || !self.owner_default_profile
            || self.proxy_ai_credentials
        {
            return Err(Failure::EnvironmentUnavailable);
        }
        Ok(())
    }
}

/// Bounded refusal evidence; provider-controlled bodies and credentials never enter it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AdmissionRejection {
    cause: AdmissionRefusal,
    observed: Option<ObservedEnvironment>,
}
impl AdmissionRejection {
    pub(crate) fn new(cause: AdmissionRefusal, observed: Option<ObservedEnvironment>) -> Self {
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
    pub(crate) fn cause(&self) -> AdmissionRefusal {
        self.cause
    }
    pub(crate) fn observed(&self) -> Option<&ObservedEnvironment> {
        self.observed.as_ref()
    }
}

#[cfg(test)]
#[path = "../../../tests/workspaces/environment.rs"]
mod tests;
