//! Channel-selected workspaces: Nessa owns placement and VM receipts; the SDK
//! owns the workspace process and file mechanisms behind infrastructure ports.
//!
//! channel -> application::Workspaces -> Settings / Assignments / Provider
//!                                      infrastructure -> workspace-sdk / Boat
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
