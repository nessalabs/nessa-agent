//! Bounded policy observation facts; no storage or effects.

use crate::domain::{
    compact_argv, valid_directory_program, DirectoryGrantState, MAX_DIRECTORY_ARGV_ITEMS,
    MAX_DIRECTORY_PATH_BYTES,
};

/// Cedar policy-port decision, not audited authority to perform an effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirectoryPolicyAnswer {
    Permit { policy_ids: Box<[Box<str>]> },
    Deny(DirectoryDenialReason),
}

/// Whether a denial was Cedar's default or cites matched forbids.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirectoryDenialReason {
    DefaultDeny,
    MatchedForbids { policy_ids: Box<[Box<str>]> },
}

/// Why the observation could not be made.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectoryPolicyError {
    InvalidFacts,
    Unavailable,
}

/// Policy for a selected grant and purported operation. The caller must
/// bind identity and actual scope and audit before any effect.
pub trait DirectoryPolicyEvaluator: Send + Sync {
    fn evaluate(
        &self,
        state: &DirectoryGrantState,
        operation: &DirectoryOperation,
    ) -> Result<DirectoryPolicyAnswer, DirectoryPolicyError>;
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
        command: DirectoryCommand,
    },
}

/// Bounded immutable command text, not a verified host/sandbox fact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectoryCommand {
    program: Box<str>,
    argv: Box<[Box<str>]>,
}

impl DirectoryCommand {
    pub fn new(program: String, argv: Vec<String>) -> Result<Self, DirectoryPolicyError> {
        if !valid_directory_program(&program)
            || argv.len() > MAX_DIRECTORY_ARGV_ITEMS
            || argv.iter().any(|arg| {
                arg.len() > MAX_DIRECTORY_PATH_BYTES || arg.chars().any(char::is_control)
            })
        {
            return Err(DirectoryPolicyError::InvalidFacts);
        }
        Ok(Self {
            program: program.into_boxed_str(),
            argv: compact_argv(argv),
        })
    }

    pub fn program(&self) -> &str {
        &self.program
    }

    pub fn argv(&self) -> &[Box<str>] {
        &self.argv
    }
}

#[cfg(test)]
mod command_capacity_tests {
    use super::*;

    #[test]
    fn command_facts_discard_spare_input_allocations() {
        let mut program = String::with_capacity(16_384);
        program.push_str("fixture");
        let mut argv = Vec::with_capacity(256);
        let mut argument = String::with_capacity(16_384);
        argument.push_str("ordinary");
        argv.push(argument);

        let command = DirectoryCommand::new(program, argv).unwrap();
        assert_eq!(command.program.into_string().capacity(), 7);
        let mut argv = command.argv.into_vec();
        assert_eq!(argv.capacity(), 1);
        assert_eq!(argv.pop().unwrap().into_string().capacity(), 8);
    }
}
