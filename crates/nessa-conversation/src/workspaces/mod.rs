//! Cloud accounts and Cloud Boxes (ADR 1417): Nessa owns the connection to
//! each cloud and each box's receipt; the SDK owns the VM mechanisms behind
//! infrastructure ports.
//!
//! cloud.boxes.* -> application::CloudBoxes -> Settings / Provider
//!                                           infrastructure -> workspace-sdk / Boat / Agent37
pub mod application;
pub mod domain;
pub mod infrastructure;

#[cfg(test)]
#[path = "../../tests/workspaces/settings.rs"]
mod settings_tests;

#[cfg(test)]
#[path = "../../tests/workspaces/provider_contract.rs"]
mod provider_contract_tests;

#[cfg(test)]
#[path = "../../tests/workspaces/boat_admission.rs"]
mod boat_admission_tests;

#[cfg(test)]
#[path = "../../tests/workspaces/agent37_provider.rs"]
mod agent37_provider_tests;
