//! Provider, encrypted settings, receipt storage and SSH lease adapters.
//! They implement the application ports; composition alone creates them.
mod agent37;
mod agent37_settings;
mod assignments;
mod boat;
mod runtime;
mod settings;
mod ssh;
pub use agent37::Agent37Provider;
pub use agent37_settings::Agent37VaultSettings;
pub use assignments::FileAssignments;
pub use boat::{vm_config, BoatProvider};
pub use settings::VaultSettings;

mod release;
pub use release::worker_image;

mod runtime_settings_audit;
pub use runtime_settings_audit::RuntimeSettingsAudit;
