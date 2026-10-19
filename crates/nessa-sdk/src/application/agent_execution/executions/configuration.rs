//! Attributed native configuration requests and their observed settlement.
use crate::application::agent_execution::{
    permissions::ActionContext,
    providers::{ApprovalMode, ModelConfiguration, ProviderIdentity},
};
use crate::domain::agent_execution::sessions::SessionId;

/// The caller's requested native control, separate from its acknowledgement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigurationControlRequest {
    /// An exact same-provider target model identity.
    Model(ProviderIdentity),
    /// A requested native permission preset.
    ApprovalMode(ApprovalMode),
}
/// Where one native configuration request stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigurationControlStage {
    /// Mandatory intent saved before dispatch.
    Requested,
    /// Complete native acknowledgement verified before the SDK settings save.
    Acknowledged,
    /// Complete native acknowledgement and SDK settings save succeeded.
    Applied,
    /// SDK-owned evidence proves no native effect.
    Refused,
    /// A native effect or its durable settlement is uncertain.
    Failed,
}
/// Owner-issued identity shared by the request and settlement records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConfigurationControl {
    session_id: SessionId,
    attachment_generation: String,
    actor: ActionContext,
    before: ProviderIdentity,
    before_configuration: ModelConfiguration,
    request: ConfigurationControlRequest,
}
impl ConfigurationControl {
    pub(crate) fn new(
        session_id: SessionId,
        attachment_generation: String,
        actor: ActionContext,
        before: ProviderIdentity,
        before_configuration: ModelConfiguration,
        request: ConfigurationControlRequest,
    ) -> Self {
        Self {
            session_id,
            attachment_generation,
            actor,
            before,
            before_configuration,
            request,
        }
    }
    pub(crate) fn record(
        &self,
        stage: ConfigurationControlStage,
        acknowledged: Option<(ProviderIdentity, ModelConfiguration)>,
    ) -> ConfigurationControlRecord {
        ConfigurationControlRecord {
            control: self.clone(),
            stage,
            acknowledged,
        }
    }
}
/// One native configuration intent or settlement, constructed only by its SDK owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigurationControlRecord {
    control: ConfigurationControl,
    stage: ConfigurationControlStage,
    acknowledged: Option<(ProviderIdentity, ModelConfiguration)>,
}
impl ConfigurationControlRecord {
    /// Local conversation receiving this control.
    pub fn session_id(&self) -> &SessionId {
        &self.control.session_id
    }
    /// Agent instance and provider generation that accepted this operation.
    pub fn attachment_generation(&self) -> &str {
        &self.control.attachment_generation
    }
    /// Original caller, window and action identity.
    pub fn actor(&self) -> &ActionContext {
        &self.control.actor
    }
    /// Exact binding in force before the request.
    pub fn before(&self) -> &ProviderIdentity {
        &self.control.before
    }
    /// Settings in force before the request.
    pub fn before_configuration(&self) -> &ModelConfiguration {
        &self.control.before_configuration
    }
    /// Original requested selection, never rewritten to a native fallback.
    pub fn request(&self) -> &ConfigurationControlRequest {
        &self.control.request
    }
    /// Where the request stands.
    pub fn stage(&self) -> ConfigurationControlStage {
        self.stage
    }
    /// Complete acknowledged binding/settings; absent until native acknowledgement is verified.
    pub fn acknowledged(&self) -> Option<(&ProviderIdentity, &ModelConfiguration)> {
        self.acknowledged
            .as_ref()
            .map(|(identity, configuration)| (identity, configuration))
    }
}
