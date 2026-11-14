//! Provider, encrypted settings, receipt storage and SSH lease adapters.
//! They implement the application ports; composition alone creates them.
mod assignments;
mod boat;
mod runtime;
mod settings;
mod ssh;
pub use assignments::FileAssignments;
pub use boat::BoatProvider;
pub use settings::VaultSettings;

mod release;
pub use release::worker_image;

mod runtime_settings_audit;
pub use runtime_settings_audit::RuntimeSettingsAudit;
