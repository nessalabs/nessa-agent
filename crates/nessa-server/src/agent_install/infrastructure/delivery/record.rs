use serde::{Deserialize, Serialize};

use super::super::audit::record::StoredTransition;
use crate::agent_install::{
    application::{InstallDeliveryFailure, InstallDeliveryFailureStage, PreparedInstallation},
    domain::{PublicationOutcome, PublicationPreparation, PublicationSettlement},
};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StoredPreparation {
    pub(super) record_id: String,
    observed_at_ms: u64,
    verified: StoredTransition,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StoredOutcome {
    pub(super) record_id: String,
    observed_at_ms: u64,
    outcome: StoredOutcomeFacts,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StoredSettlement {
    pub(super) record_id: String,
    observed_at_ms: u64,
    outcome: StoredOutcomeFacts,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredOutcomeFacts {
    Terminal { transition: StoredTransition },
    NoPublicationEffect,
}

impl StoredPreparation {
    pub(super) fn new(
        record_id: String,
        observed_at_ms: u64,
        preparation: &PublicationPreparation,
    ) -> Self {
        Self {
            record_id,
            observed_at_ms,
            verified: StoredTransition::from(preparation.verified()),
        }
    }

    pub(super) fn restore(&self) -> Result<PreparedInstallation, InstallDeliveryFailure> {
        let verified = self.verified.restore().map_err(invalid)?;
        let preparation = PublicationPreparation::new(verified).map_err(invalid)?;
        Ok(PreparedInstallation::new(
            self.record_id.clone(),
            preparation,
        ))
    }
}

impl StoredOutcome {
    pub(super) fn new(
        record_id: String,
        observed_at_ms: u64,
        outcome: &PublicationOutcome,
    ) -> Self {
        Self {
            record_id,
            observed_at_ms,
            outcome: StoredOutcomeFacts::from(outcome),
        }
    }

    pub(super) fn restore(
        &self,
        prepared: &PreparedInstallation,
    ) -> Result<PublicationOutcome, InstallDeliveryFailure> {
        if self.record_id != prepared.record_id() {
            return Err(invalid("outcome record id disagrees with its preparation"));
        }
        self.outcome.restore(prepared.preparation())
    }
}

impl StoredSettlement {
    pub(super) fn new(
        record_id: String,
        observed_at_ms: u64,
        settlement: &PublicationSettlement,
    ) -> Self {
        Self {
            record_id,
            observed_at_ms,
            outcome: StoredOutcomeFacts::from(settlement.outcome()),
        }
    }

    pub(super) fn restore(
        &self,
        prepared: &PreparedInstallation,
        retained: &PublicationOutcome,
    ) -> Result<PublicationSettlement, InstallDeliveryFailure> {
        if self.record_id != prepared.record_id() {
            return Err(invalid(
                "settlement record id disagrees with its preparation",
            ));
        }
        let outcome = self.outcome.restore(prepared.preparation())?;
        if &outcome != retained {
            return Err(invalid("settlement disagrees with its retained outcome"));
        }
        PublicationSettlement::new(prepared.preparation(), outcome).map_err(invalid)
    }
}

impl From<&PublicationOutcome> for StoredOutcomeFacts {
    fn from(value: &PublicationOutcome) -> Self {
        match value.terminal_transition() {
            Some(transition) => Self::Terminal {
                transition: StoredTransition::from(transition),
            },
            None => {
                debug_assert!(value.is_no_publication_effect());
                Self::NoPublicationEffect
            }
        }
    }
}

impl StoredOutcomeFacts {
    fn restore(
        &self,
        preparation: &PublicationPreparation,
    ) -> Result<PublicationOutcome, InstallDeliveryFailure> {
        match self {
            Self::Terminal { transition } => {
                PublicationOutcome::terminal(preparation, transition.restore().map_err(invalid)?)
                    .map_err(invalid)
            }
            Self::NoPublicationEffect => Ok(PublicationOutcome::no_publication_effect(
                preparation.clone(),
            )),
        }
    }
}

fn invalid(error: impl std::fmt::Display) -> InstallDeliveryFailure {
    InstallDeliveryFailure::new(InstallDeliveryFailureStage::ReadState, error.to_string())
}
