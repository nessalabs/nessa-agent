//! Getting an agent's own runtime onto this machine.
//!
//! Nessa drives coding agents it does not ship. Claude and Codex are expected
//! to be installed already — someone who uses them has them. Opencode is the
//! one Nessa offers to fetch, because it is the agent a first-time user can
//! reach with nothing signed in, and telling that person to go and install
//! something first is the whole of what this context exists to avoid.
//!
//! What is deliberately *not* here: keeping a runtime up to date. Nessa
//! installs the version it has tested and leaves it there. An agent that
//! updated itself underneath a tested pin would make "the version Nessa tested"
//! untrue without anything having changed on our side.
//!
//! ```text
//! composition -> infrastructure::releases_for    (which release, if any)
//!             -> application::InstallAgentRuntime -> application::ArchiveSource
//!                                                 -> application::RuntimeStore
//! infrastructure::HttpsArchives ------------------> ArchiveSource
//! infrastructure::ManagedRuntimes ----------------> RuntimeStore
//! domain::PinnedRelease --------------------------> what is allowed to be installed
//! ```
//! Arrows mean construction or calls. Dependencies point inward: the domain
//! knows nothing about the network or the disk, and the application knows only
//! the two ports.
pub mod application;
pub mod domain;
pub mod infrastructure;
