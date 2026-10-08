//! Application tests are grouped by feature in the adjacent directory.

#[path = "application/mod.rs"]
mod application;

// Shared bounded Shepherd isolation for intentional process-level faults.
#[path = "support/subprocess.rs"]
mod subprocess;
