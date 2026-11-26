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

/// The exact stored subject key. A receiver is a different key space from a principal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirectorySubject {
    /// Credential-backed unpaired principal.
    Principal(String),
    /// Paired receiver ID, not its linked principal.
    Receiver(String),
}

/// The canonical location the grantor attested in the named environment.
/// This value does not check the filesystem or prove that the path still exists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectoryResource {
    environment: String,
    canonical_path: String,
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
            environment,
            canonical_path,
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

/// Raw matcher data, not Cedar source text. [`DirectoryGrant::new`] validates
/// these fields before they become grant facts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectoryCommandRule {
    pub effect: DirectoryRuleEffect,
    pub program: String,
    /// Empty matches all argv for this program; a nonempty list matches exact arity.
    pub argv: Vec<String>,
}

impl DirectoryCommandRule {
    fn valid(&self) -> bool {
        valid_text(&self.program, MAX_DIRECTORY_PROGRAM_BYTES)
            && !self.program.contains(['/', '\\'])
            && self.argv.len() <= MAX_DIRECTORY_ARGV_ITEMS
            && self.argv.iter().all(|part| {
                part.len() <= MAX_DIRECTORY_ARGV_PATTERN_BYTES
                    && !part.chars().any(char::is_control)
            })
    }
}

/// Validated grant facts (not permission).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectoryGrant {
    capabilities: DirectoryCapabilities,
    command_rules: Vec<DirectoryCommandRule>,
}

impl DirectoryGrant {
    /// Validate the owned command rule data without interpreting any match.
    pub fn new(
        capabilities: DirectoryCapabilities,
        command_rules: Vec<DirectoryCommandRule>,
    ) -> Result<Self, InvalidDirectoryGrant> {
        if command_rules.len() > MAX_DIRECTORY_RULES
            || !command_rules.iter().all(DirectoryCommandRule::valid)
        {
            return Err(InvalidDirectoryGrant);
        }
        Ok(Self {
            capabilities,
            command_rules,
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
