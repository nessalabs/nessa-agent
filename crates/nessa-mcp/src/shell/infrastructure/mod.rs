//! Process and audit adapters. Composition injects both into the shell application.
//! Shepherd owns OS process scopes; private files retain command evidence.
mod audit;
mod runner;
pub use audit::PrivateAudit;
pub use runner::ShepherdRunner;
