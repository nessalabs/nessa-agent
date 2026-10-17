//! Provider-issued authority and acknowledged configuration for model changes.
use crate::application::agent_execution::{permissions::ActionContext, traces::LocalTraceStore};
use std::sync::Arc;

use super::{AgentProvider, ApprovalMode, ProviderIdentity};
use crate::{
    application::agent_execution::agents::AgentError,
    domain::{
        effective_capabilities::value_objects::EffectiveCapabilities,
        model_metadata::value_objects::EffortLevel,
    },
};

/// An exact same-provider transition whose factories agree on execution context.
/// `between` checks live host ownership separately from saved configuration facts;
/// the host identity is borrowed for validation and is not retained in this record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelChange {
    before: ProviderIdentity,
    after: ProviderIdentity,
    context: String,
    prior_configuration: ModelConfiguration,
    requested_configuration: ModelConfiguration,
}
impl ModelChange {
    /// Compare provider-owned context proofs without opening either provider.
    ///
    /// # Errors
    /// Refuses unsupported factories, different providers/configurations or live host owners,
    /// or an unchanged model.
    pub fn between(
        prior: &dyn AgentProvider,
        target: &dyn AgentProvider,
    ) -> Result<Self, AgentError> {
        let before = prior.identity();
        let after = target.identity();
        if before.model_id() != prior.capabilities().model().model_id()
            || after.model_id() != target.capabilities().model().model_id()
            || prior.native_hooks_allowed() != target.native_hooks_allowed()
        {
            return Err(AgentError::Configuration(
                "model change has inconsistent model or hook configuration".into(),
            ));
        }
        if prior.harness_host_identity() != target.harness_host_identity() {
            return Err(AgentError::Configuration(
                "model change selects another harness host".into(),
            ));
        }
        let context = prior
            .model_change_context()
            .ok_or_else(|| AgentError::Unsupported("model change".into()))?;
        if target.model_change_context().as_ref() != Some(&context) {
            return Err(AgentError::Configuration(
                "model change selects another execution context".into(),
            ));
        }
        let requested_configuration =
            ModelConfiguration::new(target.approval_mode(), target.effort_level());
        requested_configuration.configured_provider(target, target.traces())?;
        Self::restore(
            before,
            after,
            context,
            ModelConfiguration::new(prior.approval_mode(), prior.effort_level()),
            requested_configuration,
        )
    }
    pub(crate) fn restore(
        before: ProviderIdentity,
        after: ProviderIdentity,
        context: String,
        prior_configuration: ModelConfiguration,
        requested_configuration: ModelConfiguration,
    ) -> Result<Self, AgentError> {
        if before.name() != after.name()
            || before.model_id() == after.model_id()
            || context.is_empty()
            || context.len() > ProviderIdentity::MAX_CONTEXT_BYTES
            || context.chars().any(char::is_control)
        {
            return Err(AgentError::Configuration(
                "invalid same-provider model transition".into(),
            ));
        }
        Ok(Self {
            before,
            after,
            context,
            prior_configuration,
            requested_configuration,
        })
    }
    /// Exact identity whose saved selection this transition replaces.
    pub fn before(&self) -> &ProviderIdentity {
        &self.before
    }
    /// Exact identity selected after this transition.
    pub fn after(&self) -> &ProviderIdentity {
        &self.after
    }
    /// Owner-observed native settings before this model transition.
    pub fn prior_configuration(&self) -> &ModelConfiguration {
        &self.prior_configuration
    }
    /// Validated target factory settings for an unattached model selection.
    /// A live native acknowledgement may legitimately adjust them.
    pub fn requested_configuration(&self) -> &ModelConfiguration {
        &self.requested_configuration
    }
    pub(crate) fn context(&self) -> &str {
        &self.context
    }
}

/// Native boundary at which an acknowledged model choice can affect work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelChangeTiming {
    /// The streaming provider applies the choice to subsequent responses.
    SubsequentResponses,
    /// The current native turn retains its captured configuration.
    NextTurn,
}

/// One verified provider configuration, including native adjustments to effort or mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelChangeEffect {
    identity: ProviderIdentity,
    capabilities: EffectiveCapabilities,
    approval_mode: Option<ApprovalMode>,
    effort_level: Option<EffortLevel>,
    timing: ModelChangeTiming,
}
impl ModelChangeEffect {
    /// Construct the complete configuration verified by a provider control response.
    pub fn new(
        identity: ProviderIdentity,
        capabilities: EffectiveCapabilities,
        approval_mode: Option<ApprovalMode>,
        effort_level: Option<EffortLevel>,
        timing: ModelChangeTiming,
    ) -> Self {
        Self {
            identity,
            capabilities,
            approval_mode,
            effort_level,
            timing,
        }
    }
    /// Exact acknowledged provider/model/context identity.
    pub fn identity(&self) -> &ProviderIdentity {
        &self.identity
    }
    /// Model limits and features governing subsequent admission.
    pub fn capabilities(&self) -> &EffectiveCapabilities {
        &self.capabilities
    }
    /// Provider preset after any model-induced native adjustment.
    pub fn approval_mode(&self) -> Option<ApprovalMode> {
        self.approval_mode
    }
    /// Reasoning effort after any model-induced native adjustment.
    pub fn effort_level(&self) -> Option<&EffortLevel> {
        self.effort_level.as_ref()
    }
    /// Native application boundary; it does not relabel an existing turn.
    pub fn timing(&self) -> ModelChangeTiming {
        self.timing
    }
}

/// Durable native mode and effort; catalogue capabilities remain factory-owned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelConfiguration {
    approval_mode: Option<ApprovalMode>,
    effort_level: Option<EffortLevel>,
}
impl ModelConfiguration {
    /// Retain the complete native settings acknowledged by a configuration control.
    pub fn new(approval_mode: Option<ApprovalMode>, effort_level: Option<EffortLevel>) -> Self {
        Self {
            approval_mode,
            effort_level,
        }
    }
    pub(crate) fn configured_provider(
        &self,
        provider: &dyn AgentProvider,
        traces: Option<Arc<LocalTraceStore>>,
    ) -> Result<Arc<dyn AgentProvider>, AgentError> {
        let normalized = provider.with_model_configuration(
            self.approval_mode(),
            self.effort_level().cloned(),
            traces,
        )?;
        if normalized.identity() != provider.identity()
            || normalized.capabilities() != provider.capabilities()
            || normalized.approval_mode() != self.approval_mode()
            || normalized.effort_level().as_ref() != self.effort_level()
            || normalized.model_change_context() != provider.model_change_context()
            || normalized.harness_host_identity() != provider.harness_host_identity()
        {
            return Err(AgentError::Protocol(
                "configuration normalization changed binding or ignored settings".into(),
            ));
        }
        Ok(normalized)
    }
    /// Native preset after this configuration control.
    pub fn approval_mode(&self) -> Option<ApprovalMode> {
        self.approval_mode
    }
    /// Native reasoning effort after this configuration control.
    pub fn effort_level(&self) -> Option<&EffortLevel> {
        self.effort_level.as_ref()
    }
}
impl ModelChangeEffect {
    /// Durable settings accompanying this acknowledged model identity.
    pub fn configuration(&self) -> ModelConfiguration {
        ModelConfiguration::new(self.approval_mode, self.effort_level.clone())
    }
}

/// One saved, attributed model transition and its complete native acknowledgement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelChangeRecord {
    change: ModelChange,
    actor: ActionContext,
    configuration: ModelConfiguration,
    timing: Option<ModelChangeTiming>,
}
impl ModelChangeRecord {
    pub(crate) fn new(
        change: ModelChange,
        actor: ActionContext,
        configuration: ModelConfiguration,
        timing: Option<ModelChangeTiming>,
    ) -> Self {
        Self {
            change,
            actor,
            configuration,
            timing,
        }
    }
    pub(crate) fn retained_payload_bytes(&self) -> usize {
        let identity = |value: &ProviderIdentity| {
            value
                .name()
                .len()
                .saturating_add(value.model_id().len())
                .saturating_add(value.context().len())
        };
        identity(self.change.before())
            .saturating_add(identity(self.change.after()))
            .saturating_add(self.change.context.len())
            .saturating_add(
                self.change
                    .prior_configuration
                    .effort_level()
                    .map_or(0, |level| level.as_str().len()),
            )
            .saturating_add(
                self.change
                    .requested_configuration
                    .effort_level()
                    .map_or(0, |level| level.as_str().len()),
            )
            .saturating_add(self.actor.principal_id().len())
            .saturating_add(self.actor.surface_id().len())
            .saturating_add(self.actor.request_id().len())
            .saturating_add(
                self.configuration
                    .effort_level()
                    .map_or(0, |level| level.as_str().len()),
            )
    }
    /// Exact before/after provider identities authorized by this operation.
    pub fn change(&self) -> &ModelChange {
        &self.change
    }
    /// Host-verified actor, surface and original request identity.
    pub fn actor(&self) -> &ActionContext {
        &self.actor
    }
    /// Native application boundary; absent for an unattached deferred choice.
    pub fn timing(&self) -> Option<ModelChangeTiming> {
        self.timing
    }
    /// Native settings acknowledged with this transition.
    pub fn configuration(&self) -> &ModelConfiguration {
        &self.configuration
    }
}

/// The native setting changed by one attributed control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionConfigurationKind {
    /// A model change and all settings adjusted with it.
    Model,
    /// An explicit provider approval preset.
    ApprovalMode,
    /// An explicit reasoning effort level.
    EffortLevel,
}
/// The last durable configuration control, distinct from historical model identity changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionConfigurationRecord {
    before: Option<ModelConfiguration>,
    after: ModelConfiguration,
    actor: ActionContext,
    kind: SessionConfigurationKind,
}
impl SessionConfigurationRecord {
    pub(crate) fn new(
        before: Option<ModelConfiguration>,
        after: ModelConfiguration,
        actor: ActionContext,
        kind: SessionConfigurationKind,
    ) -> Self {
        Self {
            before,
            after,
            actor,
            kind,
        }
    }
    /// Last acknowledged settings before this control, if previously recorded.
    pub fn before(&self) -> Option<&ModelConfiguration> {
        self.before.as_ref()
    }
    /// Complete settings after this control.
    pub fn after(&self) -> &ModelConfiguration {
        &self.after
    }
    /// Original host-verified action identity.
    pub fn actor(&self) -> &ActionContext {
        &self.actor
    }
    /// Which native control established these settings.
    pub fn kind(&self) -> SessionConfigurationKind {
        self.kind
    }
    pub(crate) fn validate_relationship(
        &self,
        model: Option<&ModelChangeRecord>,
    ) -> Result<(), AgentError> {
        let valid = self.before().is_some_and(|before| match self.kind() {
            SessionConfigurationKind::ApprovalMode => {
                before.effort_level() == self.after().effort_level()
            }
            SessionConfigurationKind::EffortLevel => {
                before.approval_mode() == self.after().approval_mode()
            }
            SessionConfigurationKind::Model => model.is_some_and(|model| {
                before == model.change().prior_configuration()
                    && self.after() == model.configuration()
                    && self.actor() == model.actor()
            }),
        });
        if !valid {
            return Err(AgentError::Protocol(
                "configuration kind disagrees with retained settings or model".into(),
            ));
        }
        Ok(())
    }
    pub(crate) fn retained_payload_bytes(&self) -> usize {
        self.before
            .as_ref()
            .and_then(|configuration| configuration.effort_level())
            .map_or(0, |level| level.as_str().len())
            .saturating_add(
                self.after
                    .effort_level()
                    .map_or(0, |level| level.as_str().len()),
            )
            .saturating_add(self.actor.principal_id().len())
            .saturating_add(self.actor.surface_id().len())
            .saturating_add(self.actor.request_id().len())
    }
}
