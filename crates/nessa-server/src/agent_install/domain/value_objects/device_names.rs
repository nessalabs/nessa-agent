//! The names Windows answers to as devices rather than as files.
//!
//! Shared by every value object here that becomes a path component, because
//! this is one fact about one operating system and two copies of it would be
//! two chances to update only one. Not public beyond the domain: what leaves
//! this module is a value object that already holds the rule.

/// Names Windows reserves for devices, which it answers to in any directory and
/// with any extension.
const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Whether this string would name a device rather than a file on Windows.
///
/// The stem is what counts: `nul.txt` is the null device, not a text file, so
/// everything from the first dot onwards is disregarded. Checked on every
/// platform rather than behind `cfg`, because a value that is a directory name
/// here and a device there is not a value this domain accepts — the pin file is
/// written once and read everywhere.
pub fn names_a_device(value: &str) -> bool {
    value
        .split('.')
        .next()
        .is_some_and(|stem| RESERVED.contains(&stem))
}

#[cfg(test)]
#[path = "../../../../tests/agent_install/device_names.rs"]
mod tests;
