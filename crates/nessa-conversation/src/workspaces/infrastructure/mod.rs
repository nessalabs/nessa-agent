//! Provider, encrypted settings, cloud choice and Cloud Box adapters, for
//! Boat (`boat`, `settings`) and Agent37 (`agent37`, `agent37_settings`).
//! They implement the application ports; composition alone creates them.
mod agent37;
mod agent37_settings;
mod boat;
mod boat_callback;
mod boat_oauth;
mod cloud_boxes;
mod cloud_choice;
mod local_box_host;
mod runtime;
mod settings;
mod ssh;
pub use agent37::{Agent37Provider, SdkAgent37Enrollment};
pub use agent37_settings::Agent37VaultSettings;
pub use boat::BoatProvider;
pub use boat_callback::LoopbackCallbacks;
pub use boat_oauth::{BoatApp, BoatAppProblem, BoatOAuthHttps};
pub use cloud_boxes::{
    FileCloudBoxAudit, FileCloudBoxStore, ProviderActiveWorkChecker, WorkspaceCloudBoxProvider,
};
pub use cloud_choice::FileCloudChoice;
pub use local_box_host::{LocalBoxHost, LocalBoxSettings};
pub use settings::VaultSettings;

mod release;
pub use release::{agent37_template, boat_image};
