//! Provider, encrypted settings, cloud choice and Cloud Box adapters, for
//! Boat (`boat`, `settings`) and Agent37 (`agent37`, `agent37_settings`).
//! They implement the application ports; composition alone creates them.
mod agent37;
mod agent37_settings;
mod boat;
mod cloud_boxes;
mod cloud_choice;
mod runtime;
mod settings;
mod ssh;
pub use agent37::{Agent37Provider, SdkAgent37Enrollment};
pub use agent37_settings::Agent37VaultSettings;
pub use boat::BoatProvider;
pub use cloud_boxes::{
    FileCloudBoxAudit, FileCloudBoxStore, ProviderActiveWorkChecker, WorkspaceCloudBoxProvider,
};
pub use cloud_choice::FileCloudChoice;
pub use settings::VaultSettings;

mod release;
pub use release::{agent37_template, boat_image};
