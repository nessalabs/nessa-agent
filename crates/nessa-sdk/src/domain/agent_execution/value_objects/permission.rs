use super::PermissionOptionId;
use crate::domain::agent_execution::ExecutionError;

/// The lifetime of the requested decision, not a grant issued by this value.
/// Persistent choices require an adapter with explicit scope/persistence semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionDecision {
    AllowOnce,
    RejectOnce,
    AllowAlways,
    RejectAlways,
}

/// Which decision kinds the host permits a request to offer. Configuration does
/// not approve a tool, bypass policy, or persist a permission on its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionConfig {
    decisions: Vec<PermissionDecision>,
}
impl PermissionConfig {
    pub fn new(decisions: Vec<PermissionDecision>) -> Result<Self, ExecutionError> {
        if decisions.is_empty() {
            return Err(ExecutionError::NoPermissionOptions);
        }
        for (index, decision) in decisions.iter().enumerate() {
            if decisions[..index].contains(decision) {
                return Err(ExecutionError::DuplicatePermissionDecision);
            }
        }
        Ok(Self { decisions })
    }
    pub fn once_only() -> Self {
        Self {
            decisions: vec![
                PermissionDecision::AllowOnce,
                PermissionDecision::RejectOnce,
            ],
        }
    }
    pub fn allows(&self, decision: PermissionDecision) -> bool {
        self.decisions.contains(&decision)
    }
}

/// A specific review choice. Distinct scopes can share the same decision kind;
/// answers select the option identity, never an ambiguous boolean or kind alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionOption {
    id: PermissionOptionId,
    label: String,
    decision: PermissionDecision,
}
impl PermissionOption {
    pub fn new(
        id: PermissionOptionId,
        label: impl Into<String>,
        decision: PermissionDecision,
    ) -> Result<Self, ExecutionError> {
        let label = label.into();
        if label.trim().is_empty() {
            return Err(ExecutionError::EmptyValue("permission option label"));
        }
        Ok(Self {
            id,
            label,
            decision,
        })
    }
    pub fn id(&self) -> &PermissionOptionId {
        &self.id
    }
    pub fn label(&self) -> &str {
        &self.label
    }
    pub fn decision(&self) -> PermissionDecision {
        self.decision
    }
}

/// Nonempty, uniquely identified choices admitted by host configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionOptions(Vec<PermissionOption>);
impl PermissionOptions {
    pub fn new(
        options: Vec<PermissionOption>,
        config: &PermissionConfig,
    ) -> Result<Self, ExecutionError> {
        for (index, option) in options.iter().enumerate() {
            if options[..index].iter().any(|old| old.id() == option.id()) {
                return Err(ExecutionError::DuplicatePermissionOption);
            }
        }
        let options: Vec<_> = options
            .into_iter()
            .filter(|option| config.allows(option.decision()))
            .collect();
        if options.is_empty() {
            return Err(ExecutionError::NoPermissionOptions);
        }
        Ok(Self(options))
    }
    pub fn choices(&self) -> &[PermissionOption] {
        &self.0
    }
    pub fn find(&self, id: &PermissionOptionId) -> Option<&PermissionOption> {
        self.0.iter().find(|option| option.id() == id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermissionState {
    Pending,
    Answered {
        option_id: PermissionOptionId,
        decision: PermissionDecision,
    },
    Cancelled,
}
