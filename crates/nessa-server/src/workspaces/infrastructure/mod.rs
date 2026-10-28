//! Provider, encrypted settings, receipt storage and SSH lease adapters.
//! They implement the application ports; composition alone creates them.
mod assignments;
mod boat;
mod boat_admission;
mod runtime;
mod settings;
mod ssh;
pub(crate) use assignments::FileAssignments;
pub(crate) use boat::BoatProvider;
pub(crate) use settings::VaultSettings;

mod release;
pub(crate) use release::worker_image;

mod boat_runtime_settings;
pub(crate) use boat_runtime_settings::ConfigBoatRuntimeSettings;

mod runtime_settings_audit;
pub(crate) use runtime_settings_audit::RuntimeSettingsAudit;
