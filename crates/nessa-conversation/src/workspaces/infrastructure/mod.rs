//! Provider, encrypted settings, receipt storage, cloud choice and SSH lease
//! adapters, for Boat (`boat`, `settings`) and Agent37 (`agent37`,
//! `agent37_settings`); each cloud has its own receipt store
//! (`assignments`). They implement the application ports; composition alone
//! creates them.
mod agent37;
mod agent37_settings;
mod assignments;
mod boat;
mod cloud_boxes;
mod cloud_choice;
mod runtime;
mod settings;
mod ssh;
pub use agent37::{Agent37Provider, SdkAgent37Enrollment};
pub use agent37_settings::Agent37VaultSettings;
pub use assignments::FileAssignments;
pub use boat::{vm_config, BoatProvider};
pub use cloud_boxes::{
    FileCloudBoxStore, IdleActiveWorkChecker, TracingCloudBoxAudit, WorkspaceCloudBoxProvider,
};
pub use cloud_choice::FileCloudChoice;
pub use settings::VaultSettings;

mod release;
pub use release::{agent37_template, worker_image};

mod runtime_settings_audit;
pub use runtime_settings_audit::RuntimeSettingsAudit;
