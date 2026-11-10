//! Channel-selected workspaces: Nessa owns placement, VM receipts and the VM
//! itself; agents in the VM run over Nessa's lease protocol.
//!
//! channel -> application::Workspaces -> Settings / Assignments / Provider
//!                                      infrastructure -> Boat client
pub(crate) mod application;
pub(crate) mod domain;
pub(crate) mod infrastructure;

#[cfg(test)]
#[path = "../../tests/workspaces/allocation.rs"]
mod allocation_tests;

#[cfg(test)]
#[path = "../../tests/workspaces/assignments.rs"]
mod assignment_tests;

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
#[path = "../../tests/workspaces/runtime_settings.rs"]
mod runtime_settings_tests;
