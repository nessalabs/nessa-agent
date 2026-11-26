//! Bounded policy observation facts; no storage or effects.

/// A Cedar observation, never a permission to perform an effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectoryPolicyAnswer {
    pub permitted: bool,
    pub reasons: Vec<String>,
}

/// Why the observation could not be made.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectoryPolicyError {
    InvalidFacts,
    Unavailable,
}

/// Operation facts offered to a policy observation. The input does not prove
/// identity, path containment, or sandbox enforcement. Only a future audited
/// host owner may bind identity and scope and apply the authorized effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirectoryOperation {
    Read {
        within: bool,
    },
    Write {
        within: bool,
    },
    Exec {
        within: bool,
        enforced_boundary: bool,
        program: String,
        argv: Vec<String>,
    },
}

impl DirectoryOperation {
    pub(crate) fn valid(&self) -> bool {
        use crate::domain::*;
        match self {
            Self::Exec { program, argv, .. } => {
                !program.trim().is_empty()
                    && program.len() <= MAX_DIRECTORY_PROGRAM_BYTES
                    && !program.contains(['/', '\\'])
                    && !program.chars().any(char::is_control)
                    && argv.len() <= MAX_DIRECTORY_ARGV_ITEMS
                    && argv.iter().all(|arg| {
                        arg.len() <= MAX_DIRECTORY_PATH_BYTES && !arg.chars().any(char::is_control)
                    })
            }
            _ => true,
        }
    }
}
