//! A live model choice changes the next binding without replacing an active invocation.

use super::lifecycle::WorkPermit;
use super::{Agent, AgentError};
use crate::application::agent_execution::executions::{
    ConfigurationControl, ConfigurationControlRequest, ConfigurationControlStage,
    ExecutionAuditRecord,
};
use crate::application::agent_execution::{
    caller_wake::{contain_caller_wake, CallerWaiter},
    permissions::ActionContext,
    providers::{
        AgentProvider, ModelChange, ModelChangeEffect, ModelChangeTiming, ModelConfiguration,
        ProviderIdentity, ProviderOperationFailure, ProviderOperationResult,
        ProviderOperationStage, ProviderSessionState,
    },
};
use std::{
    future::{poll_fn, Future},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Arc,
    task::Poll,
};
impl Agent {
    pub(super) fn configuration_control(
        &self,
        permit: &WorkPermit,
        actor: ActionContext,
        request: ConfigurationControlRequest,
    ) -> ConfigurationControl {
        ConfigurationControl::new(
            self.inner.manager.id().clone(),
            format!(
                "{}:{}",
                self.inner.instance_id,
                permit.provider_generation()
            ),
            actor,
            self.provider_identity(),
            ModelConfiguration::new(self.approval_mode(), self.effort_level()),
            request,
        )
    }
    pub(super) async fn audit_configuration_control<T>(
        &self,
        permit: &WorkPermit,
        control: ConfigurationControl,
        operation: impl Future<Output = ProviderOperationResult<T>>,
        acknowledgement: impl FnOnce(&T) -> (ProviderIdentity, ModelConfiguration),
    ) -> ProviderOperationResult<T> {
        let requested = control.record(ConfigurationControlStage::Requested, None);
        let changes_configuration = match requested.request() {
            ConfigurationControlRequest::Model(_) => true,
            ConfigurationControlRequest::ApprovalMode(mode) => {
                requested.before_configuration().approval_mode() != Some(*mode)
            }
        };
        self.inner
            .lifecycle
            .record_attachment_audit(ExecutionAuditRecord::ConfigurationControlled(requested))
            .await
            .map_err(ProviderOperationFailure::before_effect)?;
        // The audit owner supervises callbacks beyond the native operation port,
        // including factory normalization, retention and final acknowledgement.
        let mut operation = Box::pin(async {
            if changes_configuration {
                permit
                    .require_no_unstarted_work()
                    .map_err(ProviderOperationFailure::before_effect)?;
            }
            let value = operation.await?;
            let acknowledged = acknowledgement(&value);
            Ok::<_, ProviderOperationFailure>((value, acknowledged))
        });
        let panicked = || {
            self.inner
                .lifecycle
                .record_control_state(permit, &ProviderSessionState::CleanupRequired);
            ProviderOperationFailure::new(
                AgentError::SubmissionUnresolved,
                ProviderSessionState::CleanupRequired,
            )
        };
        let mut result = poll_fn(|context| {
            match catch_unwind(AssertUnwindSafe(|| operation.as_mut().poll(context))) {
                Ok(poll) => poll,
                Err(payload) => {
                    std::mem::forget(payload);
                    // Fence before cleaning up the faulted future's captures.
                    Poll::Ready(Err(panicked()))
                }
            }
        })
        .await;
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| drop(operation))) {
            std::mem::forget(payload);
            let failure = panicked();
            // Preserve any already-returned primary failure when cleanup also
            // panics; either way this generation is now uncertain.
            result = Err(match result {
                Err(primary) => ProviderOperationFailure::new(
                    primary.into_error(),
                    ProviderSessionState::CleanupRequired,
                ),
                Ok(_) => failure,
            });
        }
        let result = match result {
            Err(failure) if failure.stage() == ProviderOperationStage::EffectUncertain => {
                self.inner
                    .lifecycle
                    .record_control_state(permit, &ProviderSessionState::CleanupRequired);
                Err(ProviderOperationFailure::new(
                    failure.into_error(),
                    ProviderSessionState::CleanupRequired,
                ))
            }
            result => result,
        };
        let (stage, acknowledged) = match &result {
            Ok((_, acknowledged)) => (
                ConfigurationControlStage::Applied,
                Some(acknowledged.clone()),
            ),
            Err(failure) if failure.stage() == ProviderOperationStage::BeforeEffect => {
                (ConfigurationControlStage::Refused, None)
            }
            Err(_) => (ConfigurationControlStage::Failed, None),
        };
        if let Err(audit) = self
            .inner
            .lifecycle
            .record_attachment_audit(ExecutionAuditRecord::ConfigurationControlled(
                control.record(stage, acknowledged),
            ))
            .await
        {
            self.inner
                .lifecycle
                .record_control_state(permit, &ProviderSessionState::CleanupRequired);
            let error = match result {
                Ok(_) => audit,
                Err(failure) => AgentError::MultipleOperationFailures {
                    first_error: Box::new(failure.into_error()),
                    subsequent_error: Box::new(audit),
                },
            };
            return Err(ProviderOperationFailure::new(
                error,
                ProviderSessionState::CleanupRequired,
            ));
        }
        result.map(|(value, _)| value)
    }
    pub(super) async fn acknowledge_configuration(
        &self,
        permit: &WorkPermit,
        control: &ConfigurationControl,
        identity: ProviderIdentity,
        configuration: ModelConfiguration,
    ) -> ProviderOperationResult<()> {
        self.inner
            .lifecycle
            .record_attachment_audit(ExecutionAuditRecord::ConfigurationControlled(
                control.record(
                    ConfigurationControlStage::Acknowledged,
                    Some((identity, configuration)),
                ),
            ))
            .await
            .map_err(|error| {
                self.inner
                    .lifecycle
                    .record_control_state(permit, &ProviderSessionState::CleanupRequired);
                ProviderOperationFailure::new(error, ProviderSessionState::CleanupRequired)
            })
    }
    pub(super) fn configured_provider(
        &self,
        configuration: &ModelConfiguration,
    ) -> Result<Arc<dyn AgentProvider>, AgentError> {
        let provider = self.current_provider();
        if provider.approval_mode() == configuration.approval_mode()
            && provider.effort_level().as_ref() == configuration.effort_level()
        {
            return Ok(provider);
        }
        configuration.configured_provider(provider.as_ref(), self.inner.traces.clone())
    }
    pub(super) fn retain_configuration(
        &self,
        configuration: &ModelConfiguration,
    ) -> Result<(), AgentError> {
        let provider = self.configured_provider(configuration)?;
        *self.inner.provider.write().expect("current model binding") = provider;
        Ok(())
    }
    /// Apply and verify a same-provider model choice on the existing native session.
    /// The provider decides the application boundary, returned in the complete
    /// acknowledged configuration. Existing turn IDs and trace identity remain intact.
    /// The owned task settles even if the caller drops its wait. Admissions and
    /// other configuration controls are serialized by the scheduler lock.
    ///
    /// # Errors
    /// Unstarted admitted input, including queued, selected or preparing work,
    /// returns [`AgentError::Busy`] before native effect. A native-executing turn
    /// alone still allows supported model control.
    /// Unsupported or different-context targets are refused before sending.
    /// Provider, persistence and close failures carry explicit cleanup state;
    /// an uncertain generation cannot authorize subsequent work. If an admitted
    /// owner panics, even during preflight before native entry, the returned
    /// [`AgentError::SubmissionUnresolved`] carries
    /// [`ProviderSessionState::CleanupRequired`] and that generation refuses new
    /// work until cleanup. This does not confirm native application.
    pub async fn set_model(
        &self,
        target: Arc<dyn AgentProvider>,
        actor: ActionContext,
    ) -> ProviderOperationResult<ModelChangeEffect> {
        let agent = self.clone();
        contain_caller_wake(
            CallerWaiter::ModelChange(self.inner.manager.id().clone()),
            tokio::spawn(async move { agent.change_model(target, actor).await }),
        )
        .await
        .unwrap_or_else(|_| {
            Err(ProviderOperationFailure::new(
                AgentError::SubmissionUnresolved,
                ProviderSessionState::CleanupRequired,
            ))
        })
    }
    async fn change_model(
        &self,
        target: Arc<dyn AgentProvider>,
        actor: ActionContext,
    ) -> ProviderOperationResult<ModelChangeEffect> {
        let _scheduler = self.inner.scheduler.lock().await;
        let permit = self
            .inner
            .lifecycle
            .accept_control()
            .map_err(ProviderOperationFailure::before_effect)?;
        let control = self.configuration_control(
            &permit,
            actor.clone(),
            ConfigurationControlRequest::Model(target.identity()),
        );
        self.audit_configuration_control(
            &permit,
            control.clone(),
            self.change_model_on(&permit, &control, target, actor),
            |effect| (effect.identity().clone(), effect.configuration()),
        )
        .await
    }
    async fn change_model_on(
        &self,
        permit: &WorkPermit,
        control: &ConfigurationControl,
        target: Arc<dyn AgentProvider>,
        actor: ActionContext,
    ) -> ProviderOperationResult<ModelChangeEffect> {
        let prior = self.current_provider();
        let refused = ProviderOperationFailure::before_effect;
        let change = ModelChange::between(prior.as_ref(), target.as_ref()).map_err(refused)?;
        let active_initiation = if let Some(id) = self.inner.lifecycle.active() {
            self.inner.manager.initiation_of(&id).await
        } else {
            None
        };
        if let Some(initiation) = &active_initiation {
            super::agent::checked_tools_unsupported(
                initiation,
                target.approval_mode(),
                self.operation_capabilities(),
            )
            .map_err(refused)?;
        }
        let attached = self
            .inner
            .lifecycle
            .attached_provider(permit)
            .map_err(refused)?;
        let observed = self
            .inner
            .lifecycle
            .run_control_observed(
                permit,
                attached
                    .session
                    .set_model(change.clone(), target.capabilities().clone()),
            )
            .await;
        let effect = match observed {
            Ok(effect) => effect,
            Err(failure) => {
                self.inner
                    .lifecycle
                    .record_control_state(permit, &ProviderSessionState::CleanupRequired);
                let (error, _) = failure.into_parts();
                return Err(ProviderOperationFailure::new(
                    error,
                    ProviderSessionState::CleanupRequired,
                ));
            }
        };
        let verified = (|| {
            if effect.identity() != change.after() || effect.capabilities() != target.capabilities()
            {
                return Err(AgentError::Protocol(
                    "model control acknowledged another configuration".into(),
                ));
            }
            if let Some(initiation) = &active_initiation {
                super::agent::checked_tools_unsupported(
                    initiation,
                    effect.approval_mode(),
                    self.operation_capabilities(),
                )?;
            }
            if effect.effort_level().is_some_and(|level| {
                !target
                    .capabilities()
                    .effort_levels()
                    .and_then(|levels| {
                        levels.restricted_to(self.operation_capabilities().effort_levels())
                    })
                    .is_some_and(|levels| levels.contains(level))
            }) {
                return Err(AgentError::Protocol(
                    "model control acknowledged an effort outside model capabilities".into(),
                ));
            }
            effect
                .configuration()
                .configured_provider(target.as_ref(), self.inner.traces.clone())
        })();
        let normalized = match verified {
            Ok(provider) => provider,
            Err(error) => {
                self.inner
                    .lifecycle
                    .record_control_state(permit, &ProviderSessionState::CleanupRequired);
                return Err(ProviderOperationFailure::new(
                    error,
                    ProviderSessionState::CleanupRequired,
                ));
            }
        };
        self.acknowledge_configuration(
            permit,
            control,
            effect.identity().clone(),
            effect.configuration(),
        )
        .await?;
        if let Err(error) = self
            .inner
            .manager
            .record_model_change(change, effect.configuration(), effect.timing(), actor)
            .await
        {
            self.inner
                .lifecycle
                .record_control_state(permit, &ProviderSessionState::CleanupRequired);
            return Err(ProviderOperationFailure::new(
                AgentError::Storage(error),
                ProviderSessionState::CleanupRequired,
            ));
        }
        match effect.timing() {
            ModelChangeTiming::NextTurn => {}
            ModelChangeTiming::SubsequentResponses => self
                .inner
                .lifecycle
                .acknowledge_active_configuration(normalized.clone()),
        }
        *self.inner.provider.write().expect("current model binding") = normalized;
        Ok(effect)
    }
}
