//! Provider, encrypted settings, receipt storage and managed-pipe adapters.
//! They implement the application ports; composition alone creates them.
mod assignments;
mod boat;
mod runtime;
mod settings;
mod ssh;
pub(crate) use assignments::FileAssignments;
pub(crate) use boat::BoatProvider;
pub(crate) use settings::VaultSettings;

mod release;
pub(crate) use release::worker_image;
