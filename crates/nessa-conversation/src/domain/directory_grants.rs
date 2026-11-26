//! Directory grant facts, independent of policy evaluation and filesystem I/O.

/// Maximum UTF-8 bytes in a host-attested canonical path.
pub const MAX_DIRECTORY_PATH_BYTES: usize = 4096;
/// Maximum command rules in a grant.
pub const MAX_DIRECTORY_RULES: usize = 16;
/// Maximum UTF-8 bytes in a program's file name.
pub const MAX_DIRECTORY_PROGRAM_BYTES: usize = 128;
/// Maximum argument positions in one command matcher.
pub const MAX_DIRECTORY_ARGV_ITEMS: usize = 8;
/// Maximum UTF-8 bytes in each argument pattern.
pub const MAX_DIRECTORY_ARGV_PATTERN_BYTES: usize = 128;
/// Maximum UTF-8 bytes in a durable grant selector.
pub const MAX_DIRECTORY_SELECTOR_BYTES: usize = 256;

/// The subject's identity space; equal text in different spaces is distinct.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectorySubjectKind {
    Principal,
    Receiver,
}

/// Validated, compact immutable selector for one kind of subject.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectorySubject {
    kind: DirectorySubjectKind,
    id: Box<str>,
}

impl DirectorySubject {
    pub fn new(kind: DirectorySubjectKind, id: String) -> Result<Self, InvalidDirectoryGrant> {
        if !selector_valid(&id) {
            return Err(InvalidDirectoryGrant);
        }
        Ok(Self {
            kind,
            id: id.into_boxed_str(),
        })
    }

    pub fn kind(&self) -> DirectorySubjectKind {
        self.kind
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

/// The canonical location the grantor attested in the named environment.
/// This value does not check the filesystem or prove that the path still exists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectoryResource {
    environment: Box<str>,
    canonical_path: Box<str>,
}

impl DirectoryResource {
    /// Store an already resolved path; never resolve a path in this value type.
    pub fn from_canonical(
        environment: String,
        canonical_path: String,
    ) -> Result<Self, InvalidDirectoryGrant> {
        if !selector_valid(&environment) || !valid_text(&canonical_path, MAX_DIRECTORY_PATH_BYTES) {
            return Err(InvalidDirectoryGrant);
        }
        Ok(Self {
            environment: environment.into_boxed_str(),
            canonical_path: canonical_path.into_boxed_str(),
        })
    }

    /// Environment that attested the path.
    pub fn environment(&self) -> &str {
        &self.environment
    }

    /// Exact canonical path in that environment.
    pub fn canonical_path(&self) -> &str {
        &self.canonical_path
    }
}

pub(crate) fn selector_valid(value: &str) -> bool {
    valid_text(value, MAX_DIRECTORY_SELECTOR_BYTES)
}

fn valid_text(value: &str, bound: usize) -> bool {
    !value.trim().is_empty() && value.len() <= bound && !value.chars().any(char::is_control)
}

/// A directory capability. Authorization is decided separately by Cedar.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectoryCapability {
    /// Read a path within the directory.
    Read,
    /// Write a path within the directory.
    Write,
    /// Run a command within an independently enforced boundary.
    Exec,
}

/// A validated nonempty subset of directory capabilities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirectoryCapabilities(u8);

impl DirectoryCapabilities {
    /// Construct a nonempty set.
    pub fn new(values: &[DirectoryCapability]) -> Result<Self, InvalidDirectoryGrant> {
        let bits = values.iter().fold(0, |bits, capability| {
            bits | match capability {
                DirectoryCapability::Read => 1,
                DirectoryCapability::Write => 2,
                DirectoryCapability::Exec => 4,
            }
        });
        Self::from_bits(bits)
    }

    pub(crate) fn from_bits(bits: u8) -> Result<Self, InvalidDirectoryGrant> {
        if (1..=7).contains(&bits) {
            Ok(Self(bits))
        } else {
            Err(InvalidDirectoryGrant)
        }
    }

    pub(crate) fn facts(self) -> [bool; 3] {
        [self.0 & 1 != 0, self.0 & 2 != 0, self.0 & 4 != 0]
    }
}

/// The effect of one stored matcher. Its precedence is owned by Cedar.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectoryRuleEffect {
    /// Permit a matching command.
    Allow,
    /// Forbid a matching command.
    Deny,
}

/// Validated matcher facts, not Cedar source text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectoryCommandRule {
    effect: DirectoryRuleEffect,
    program: Box<str>,
    argv: Box<[Box<str>]>,
}

impl DirectoryCommandRule {
    /// Empty argv matches all arguments; nonempty matches exact arity.
    pub fn new(
        effect: DirectoryRuleEffect,
        program: String,
        argv: Vec<String>,
    ) -> Result<Self, InvalidDirectoryGrant> {
        if !valid_directory_program(&program)
            || argv.len() > MAX_DIRECTORY_ARGV_ITEMS
            || argv.iter().any(|part| {
                part.len() > MAX_DIRECTORY_ARGV_PATTERN_BYTES
                    || part.chars().any(char::is_control)
            })
        {
            return Err(InvalidDirectoryGrant);
        }
        Ok(Self {
            effect,
            program: program.into_boxed_str(),
            argv: compact_argv(argv),
        })
    }

    pub fn effect(&self) -> DirectoryRuleEffect {
        self.effect
    }

    pub fn program(&self) -> &str {
        &self.program
    }

    pub fn argv(&self) -> &[Box<str>] {
        &self.argv
    }
}

pub(crate) fn valid_directory_program(value: &str) -> bool {
    valid_text(value, MAX_DIRECTORY_PROGRAM_BYTES) && !value.contains(['/', '\\'])
}

pub(crate) fn compact_argv(values: Vec<String>) -> Box<[Box<str>]> {
    values.into_iter().map(String::into_boxed_str).collect()
}

/// Validated grant facts (not permission).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectoryGrant {
    capabilities: DirectoryCapabilities,
    command_rules: Box<[DirectoryCommandRule]>,
}

impl DirectoryGrant {
    /// Validate the owned command rule data without interpreting any match.
    pub fn new(
        capabilities: DirectoryCapabilities,
        command_rules: Vec<DirectoryCommandRule>,
    ) -> Result<Self, InvalidDirectoryGrant> {
        if command_rules.len() > MAX_DIRECTORY_RULES {
            return Err(InvalidDirectoryGrant);
        }
        Ok(Self {
            capabilities,
            command_rules: command_rules.into_boxed_slice(),
        })
    }

    /// Durable capability facts. Cedar decides whether they allow the request.
    pub fn capabilities(&self) -> DirectoryCapabilities {
        self.capabilities
    }

    /// Ordered durable matcher data.
    pub fn command_rules(&self) -> &[DirectoryCommandRule] {
        &self.command_rules
    }
}

/// Structurally invalid directory grant data, not a policy refusal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidDirectoryGrant;

/// Grant lifecycle facts. Never holding a grant differs from revocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirectoryGrantState {
    Absent,
    Live {
        grant: DirectoryGrant,
        revision: i64,
    },
    Revoked {
        revision: i64,
    },
}

#[cfg(test)]
mod retained_capacity_tests {
    use super::*;

    fn spare(text: &str) -> String {
        let mut retained = String::with_capacity(16_384);
        retained.push_str(text);
        retained
    }

    #[test]
    fn directory_value_objects_discard_spare_input_allocations() {
        let subject = DirectorySubject::new(DirectorySubjectKind::Principal, spare("id")).unwrap();
        assert_eq!(subject.id.into_string().capacity(), 2);

        let resource = DirectoryResource::from_canonical(spare("host"), spare("/fixture")).unwrap();
        assert_eq!(resource.environment.into_string().capacity(), 4);
        assert_eq!(resource.canonical_path.into_string().capacity(), 8);

        let mut args = Vec::with_capacity(256);
        args.push(spare("arg"));
        let mut rules = Vec::with_capacity(256);
        rules.push(
            DirectoryCommandRule::new(DirectoryRuleEffect::Allow, spare("program"), args)
                .unwrap(),
        );
        let rights = DirectoryCapabilities::new(&[DirectoryCapability::Exec]).unwrap();
        let grant = DirectoryGrant::new(rights, rules).unwrap();
        let mut rules = grant.command_rules.into_vec();
        assert_eq!(rules.capacity(), 1);
        let rule = rules.pop().unwrap();
        assert_eq!(rule.program.into_string().capacity(), 7);
        let mut args = rule.argv.into_vec();
        assert_eq!(args.capacity(), 1);
        assert_eq!(args.pop().unwrap().into_string().capacity(), 3);
    }
}
