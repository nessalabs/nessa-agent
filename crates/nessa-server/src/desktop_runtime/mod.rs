//! Desktop runtime identity and acknowledged gateway retirement.
//! Domain requests -> application retirement -> private infrastructure records.
//! The server remains alive after retirement; the desktop's native service
//! manager owns process replacement.
//! Runtime content, installed service generation and process incarnation have
//! separate validated identities. Admitted retirement result files fence their
//! generation before startup admits product work, including when cleanup or
//! audit acknowledgement failed. The replacement generation remains eligible
//! even when it uses identical runtime content.
pub(crate) mod application;
pub(crate) mod domain;
pub(crate) mod infrastructure;
