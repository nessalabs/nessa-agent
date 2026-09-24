//! Immutable outcome of one automatic runtime preparation.
//!
//! The projection keeps four facts independent: what preparation established,
//! whether process and executable-use ownership was released, whether audit was
//! acknowledged, and what happened to the durable completion record. Provider
//! diagnostics remain in the warm-up audit; composition uses only the ownership
//! fact to decide whether another automatic launch is safe.

use crate::agent_warm_up::domain::RuntimeFingerprint;

/// What the automatic preparation established about this runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WarmUpEffect {
    /// A durable record already said this runtime was prepared.
    AlreadyPrepared,
    /// This run opened and closed a provider session.
    Prepared,
    /// This run reached a known failure.
    Failed,
    /// The worker was lost, so its effect cannot be determined.
    Unknown,
}

/// Whether this run can still own a process or executable-use generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WarmUpLaunchOwnership {
    /// No launch resources were acquired, or their release was confirmed.
    Released,
    /// Release was not confirmed; another automatic launch must remain fenced.
    Retained,
}

/// Delivery of the automatic warm-up's own audit evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WarmUpAuditDelivery {
    /// No transition reached the point at which audit was required.
    NotAttempted,
    /// The audit sink acknowledged the transition.
    Acknowledged,
    /// The audit sink rejected the transition.
    Rejected,
    /// Worker loss made delivery unknowable.
    Unknown,
}

/// Delivery of the durable runtime-preparation record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WarmUpCompletionRecordDelivery {
    /// An existing record matched this runtime; this run acquired no resources.
    Existing,
    /// This run committed a new completion record.
    Recorded,
    /// The run did not qualify to write a completion record.
    NotAttempted,
    /// Reading the existing record was rejected.
    ReadRejected,
    /// Writing the new completion record was rejected.
    WriteRejected,
    /// Worker loss made record delivery unknowable.
    Unknown,
}

/// Immutable terminal projection for one automatic runtime preparation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WarmUpTerminal {
    runtime: RuntimeFingerprint,
    effect: WarmUpEffect,
    launch_ownership: WarmUpLaunchOwnership,
    audit_delivery: WarmUpAuditDelivery,
    completion_record_delivery: WarmUpCompletionRecordDelivery,
}

impl WarmUpTerminal {
    pub(super) fn new(
        runtime: RuntimeFingerprint,
        effect: WarmUpEffect,
        launch_ownership: WarmUpLaunchOwnership,
        audit_delivery: WarmUpAuditDelivery,
        completion_record_delivery: WarmUpCompletionRecordDelivery,
    ) -> Self {
        Self {
            runtime,
            effect,
            launch_ownership,
            audit_delivery,
            completion_record_delivery,
        }
    }

    /// Runtime whose automatic preparation settled.
    pub(super) fn runtime(&self) -> &RuntimeFingerprint {
        &self.runtime
    }

    /// What preparation established.
    pub const fn effect(&self) -> WarmUpEffect {
        self.effect
    }

    /// Whether another automatic launch may safely acquire process/use ownership.
    pub const fn launch_ownership(&self) -> WarmUpLaunchOwnership {
        self.launch_ownership
    }

    /// Whether the warm-up transition's audit was acknowledged.
    pub const fn audit_delivery(&self) -> WarmUpAuditDelivery {
        self.audit_delivery
    }

    /// What happened to the durable completion record.
    pub const fn completion_record_delivery(&self) -> WarmUpCompletionRecordDelivery {
        self.completion_record_delivery
    }
}
