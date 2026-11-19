//! A directory grant's capabilities and command matchers (issue 892).
//!
//! What those capabilities allow is Cedar's. This module checks shape and
//! size, and names the file-name and argv facts a command matcher is compared
//! with. It does not decide a path, a program, or a sandbox.

use super::command_policy::program_name;

/// The marker file's name. The filesystem entry is the check; this spelling
/// is what the grantor creates.
pub const MARKER_NAME: &str = ".nessa-binding";

/// Bytes in a directory binding.
pub const BINDING_LEN: usize = 32;

/// The product frame a share result has to fit in.
pub const PRODUCT_FRAME_BYTES: usize = 65_536;

/// Bytes the share frame uses around the encoded grant.
pub const SHARE_FRAME_OVERHEAD_BYTES: usize = 1_024;

/// The most bytes an encoded directory grant may be.
pub const MAX_ENCODED_GRANT_BYTES: usize = PRODUCT_FRAME_BYTES - SHARE_FRAME_OVERHEAD_BYTES;

/// How a command matcher treats a program it matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleEffect {
    /// The program may run when no deny matcher also matches.
    Allow,
    /// The program is refused. A deny wins over an allow.
    Deny,
}

/// A bounded argv pattern: `*` matches any run of characters, and every other
/// character is itself. The text is the arguments joined by U+001F.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArgvPattern(String);

impl ArgvPattern {
    /// The longest pattern, in UTF-8 bytes.
    pub const MAX_BYTES: usize = 256;

    /// `text` when it is a pattern a matcher can store.
    pub fn new(text: impl Into<String>) -> Result<Self, DirectoryShapeError> {
        let text = text.into();
        if text.is_empty()
            || text.len() > Self::MAX_BYTES
            || text.trim() != text
            || text.chars().any(char::is_control)
        {
            return Err(DirectoryShapeError::ArgvPattern);
        }
        Ok(Self(text))
    }

    /// The pattern as stored.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether `argv` matches this pattern. `argv` is the joined arguments.
    pub fn matches(&self, argv: &str) -> bool {
        like(&self.0, argv)
    }
}

/// One matcher in the grant's ordered list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandMatcher {
    effect: RuleEffect,
    program: String,
    argv: Option<ArgvPattern>,
}

impl CommandMatcher {
    /// The longest program file name, in bytes.
    pub const MAX_PROGRAM_BYTES: usize = 255;

    /// A matcher for the file name `program`, and `argv` when the matcher
    /// cares about arguments.
    pub fn new(
        effect: RuleEffect,
        program: impl Into<String>,
        argv: Option<ArgvPattern>,
    ) -> Result<Self, DirectoryShapeError> {
        let program = program.into();
        if program.is_empty()
            || program.len() > Self::MAX_PROGRAM_BYTES
            || program.contains('/')
            || program.contains('\\')
            || program.trim() != program
            || program.chars().any(char::is_control)
        {
            return Err(DirectoryShapeError::Program);
        }
        Ok(Self {
            effect,
            program,
            argv,
        })
    }

    /// Allow or deny.
    pub fn effect(&self) -> RuleEffect {
        self.effect
    }

    /// The program file name.
    pub fn program(&self) -> &str {
        &self.program
    }

    /// The argv pattern, when the matcher has one.
    pub fn argv(&self) -> Option<&ArgvPattern> {
        self.argv.as_ref()
    }

    /// Whether this matcher names `program`'s file name and, when it has a
    /// pattern, the joined `argv`.
    pub fn matches(&self, program: &str, argv: &str) -> bool {
        if program_name(program) != self.program {
            return false;
        }
        self.argv
            .as_ref()
            .is_none_or(|pattern| pattern.matches(argv))
    }
}

/// The ordered allow and deny matchers stored on one directory grant.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CommandRules {
    rules: Vec<CommandMatcher>,
    /// The stored text was not a matcher list. Checked as a refusal.
    unreadable: bool,
}

impl CommandRules {
    /// How many matchers one grant holds.
    pub const MAX_RULES: usize = 16;

    /// No extra command constraint.
    pub fn none() -> Self {
        Self {
            rules: Vec::new(),
            unreadable: false,
        }
    }

    /// Stored matcher text that could not be read. It allows nothing.
    pub fn unreadable() -> Self {
        Self {
            rules: Vec::new(),
            unreadable: true,
        }
    }

    /// `rules` in the order the owner gave them.
    pub fn new(rules: Vec<CommandMatcher>) -> Result<Self, DirectoryShapeError> {
        if rules.len() > Self::MAX_RULES {
            return Err(DirectoryShapeError::TooManyRules);
        }
        Ok(Self {
            rules,
            unreadable: false,
        })
    }

    /// The matchers, in order.
    pub fn rules(&self) -> &[CommandMatcher] {
        &self.rules
    }

    /// Whether any matcher is a constraint a call could skip.
    pub fn constrains_commands(&self) -> bool {
        !self.rules.is_empty()
    }

    /// The stored list could not be read, so no command is allowed.
    pub fn is_unreadable(&self) -> bool {
        self.unreadable
    }
}

/// Capabilities a directory grant may name. Any non-empty subset of read,
/// write, and exec. Order in the stored spelling is read, then write, then
/// exec.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DirectoryCapabilities {
    read: bool,
    write: bool,
    exec: bool,
}

impl DirectoryCapabilities {
    /// The set named by `names`, in any order.
    pub fn from_names<'a>(
        names: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, DirectoryShapeError> {
        let mut read = false;
        let mut write = false;
        let mut exec = false;
        for name in names {
            match name {
                "read" if !read => read = true,
                "write" if !write => write = true,
                "exec" if !exec => exec = true,
                _ => return Err(DirectoryShapeError::Capabilities),
            }
        }
        if !read && !write && !exec {
            return Err(DirectoryShapeError::Capabilities);
        }
        Ok(Self { read, write, exec })
    }

    /// Whether the set includes `name` (`read`, `write`, or `exec`).
    pub fn contains(self, name: &str) -> bool {
        match name {
            "read" => self.read,
            "write" => self.write,
            "exec" => self.exec,
            _ => false,
        }
    }

    /// The stored spelling.
    pub fn as_str(self) -> String {
        let mut names = Vec::new();
        if self.read {
            names.push("read");
        }
        if self.write {
            names.push("write");
        }
        if self.exec {
            names.push("exec");
        }
        names.join("+")
    }

    /// The spelling [`Self::as_str`] wrote.
    pub fn parse(text: &str) -> Option<Self> {
        Self::from_names(text.split('+'))
            .ok()
            .filter(|parsed| parsed.as_str() == text)
    }
}

/// Why a directory value was refused before any policy question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectoryShapeError {
    /// The capability set was empty or named something twice or unknown.
    Capabilities,
    /// The program was not a single file name.
    Program,
    /// The argv pattern was empty, too long, or had a control character.
    ArgvPattern,
    /// The matcher list was longer than [`CommandRules::MAX_RULES`].
    TooManyRules,
    /// The encoded grant would not fit in one product frame.
    Frame,
}

/// Join command arguments the way an argv pattern sees them.
pub fn join_argv(argv: &[impl AsRef<str>]) -> String {
    argv.iter()
        .map(AsRef::as_ref)
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

/// The encoded grant a share frame has to carry.
pub fn encoded_grant(
    environment: &str,
    canonical_path: &str,
    capabilities: DirectoryCapabilities,
    rules: &CommandRules,
) -> Result<String, DirectoryShapeError> {
    let mut matchers = String::from("[");
    for (index, rule) in rules.rules().iter().enumerate() {
        if index > 0 {
            matchers.push(',');
        }
        let effect = match rule.effect() {
            RuleEffect::Allow => "allow",
            RuleEffect::Deny => "deny",
        };
        matchers.push_str("{\"effect\":");
        matchers.push_str(&quote(effect));
        matchers.push_str(",\"program\":");
        matchers.push_str(&quote(rule.program()));
        if let Some(argv) = rule.argv() {
            matchers.push_str(",\"argv\":");
            matchers.push_str(&quote(argv.as_str()));
        }
        matchers.push('}');
    }
    matchers.push(']');
    let mut encoded = String::from("{\"environment\":");
    encoded.push_str(&quote(environment));
    encoded.push_str(",\"path\":");
    encoded.push_str(&quote(canonical_path));
    encoded.push_str(",\"capabilities\":");
    encoded.push_str(&quote(&capabilities.as_str()));
    encoded.push_str(",\"commandRules\":");
    encoded.push_str(&matchers);
    encoded.push('}');
    if encoded.len() > MAX_ENCODED_GRANT_BYTES {
        return Err(DirectoryShapeError::Frame);
    }
    Ok(encoded)
}

/// What a no-follow walk observed. These are facts for Cedar, not a verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathLeave {
    /// Every component opened under the parent handle.
    Inside,
    /// A component was `..`.
    DotDot,
    /// A component was a symlink.
    Symlink,
    /// A component left the granted directory some other way.
    Outside,
    /// A component was not there.
    Missing,
}

impl PathLeave {
    /// The word Cedar reads.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inside => "inside",
            Self::DotDot => "dotdot",
            Self::Symlink => "symlink",
            Self::Outside => "outside",
            Self::Missing => "missing",
        }
    }
}

/// The walk from the granted directory, and whether the opened entry is the
/// marker file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WalkFacts {
    /// How the walk ended.
    pub leave: PathLeave,
    /// The opened directory entry is the marker, including a hard link or a
    /// case-folded alias of that same file.
    pub marker: bool,
}

/// A sandbox profile the environment says it can enforce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxKind {
    /// Nothing encloses the process.
    None,
    /// The harness sandbox.
    Harness,
    /// A container.
    Container,
}

impl SandboxKind {
    /// The word Cedar and the audit record use.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Harness => "harness",
            Self::Container => "container",
        }
    }
}

fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if other.is_control() => {
                out.push_str(&format!("\\u{:04x}", u32::from(other)));
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn like(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut rest = text;
    if let Some(first) = parts.first().filter(|part| !part.is_empty()) {
        let Some(stripped) = rest.strip_prefix(first) else {
            return false;
        };
        rest = stripped;
    }
    if let Some(last) = parts.last().filter(|part| !part.is_empty()) {
        let Some(stripped) = rest.strip_suffix(last) else {
            return false;
        };
        rest = stripped;
    }
    let mut from = 0;
    for part in parts.iter().skip(1).take(parts.len().saturating_sub(2)) {
        if part.is_empty() {
            continue;
        }
        let Some(found) = rest[from..].find(part) else {
            return false;
        };
        from += found + part.len();
    }
    true
}

#[cfg(test)]
#[path = "../../tests/conversation/directory_grant_shape.rs"]
mod tests;
