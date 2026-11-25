use std::{error::Error, fmt};

/// A typed refusal when constructing, serving, or verifying an MCP skill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpSkillError {
    /// Skill name violates Agent Skills naming rules (1..=64 lowercase ASCII
    /// alphanumeric or `-`, no leading/trailing `-`, no `--`).
    InvalidSkillName,
    /// Skill URI is empty, too long, contains invalid characters, or does not
    /// end in `/<name>/SKILL.md`.
    InvalidSkillUri,
    /// Resource file URI is empty, too long, or contains invalid characters.
    InvalidFileUri,
    /// Relative path escapes the skill root or contains invalid segments.
    InvalidRelativePath,
    /// Digest is not `sha256:{64 lowercase hex}`.
    InvalidDigest,
    /// `SKILL.md` lacks valid YAML frontmatter or failed to parse.
    InvalidFrontmatter(String),
    /// A required frontmatter field (`name` or `description`) is absent or has
    /// the wrong type.
    MissingFrontmatterField(&'static str),
    /// The final path segment of the skill URI does not equal `frontmatter.name`.
    NameMismatch {
        /// Skill name derived from the URI.
        uri_name: String,
        /// Skill name declared in YAML frontmatter.
        frontmatter_name: String,
    },
    /// A manifest `resources` array is empty.
    EmptyManifest,
    /// A manifest does not include an entry for the skill's `SKILL.md` URI.
    MissingSkillMdInManifest,
    /// A manifest lists the same file URI more than once.
    DuplicateManifestUri(String),
    /// A manifest entry URI does not sit inside the skill's root directory.
    ManifestFileOutsideSkillRoot(String),
    /// A skill manifest exceeds SEP-2640's file count limit.
    TooManyFiles {
        /// Number of files in the manifest.
        count: usize,
        /// Maximum permitted files.
        max: usize,
    },
    /// A skill manifest exceeds SEP-2640's total byte size limit.
    TotalSizeExceeded {
        /// Sum of declared file sizes in bytes.
        bytes: u64,
        /// Maximum permitted bytes.
        max: u64,
    },
    /// Attempted to read or verify a file URI not present in the retained manifest.
    UnlistedResource(String),
    /// Retrieved file byte length does not match the manifest entry's `size`.
    SizeMismatch {
        /// Expected size in bytes from the manifest.
        expected: u64,
        /// Actual byte length read.
        actual: u64,
    },
    /// Retrieved file SHA-256 digest does not match the manifest entry's `digest`.
    DigestMismatch {
        /// Expected digest from the manifest.
        expected: String,
        /// Computed digest of the retrieved bytes.
        actual: String,
    },
    /// Retrieved `SKILL.md` frontmatter does not match the retained entry's
    /// frontmatter field-by-field.
    FrontmatterMismatch,
    /// Directory URI is malformed (for example, has a trailing slash).
    InvalidDirectoryUri(String),
    /// Directory URI does not exist in the skill or catalog.
    DirectoryNotFound(String),
    /// Two skills in a catalog share the same `SKILL.md` URI.
    DuplicateSkillUri(String),
}

impl fmt::Display for McpSkillError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSkillName => f.write_str("invalid skill name"),
            Self::InvalidSkillUri => f.write_str("invalid skill URI"),
            Self::InvalidFileUri => f.write_str("invalid skill file URI"),
            Self::InvalidRelativePath => f.write_str("invalid relative skill file path"),
            Self::InvalidDigest => f.write_str("invalid sha256 skill digest"),
            Self::InvalidFrontmatter(reason) => write!(f, "invalid SKILL.md frontmatter: {reason}"),
            Self::MissingFrontmatterField(field) => {
                write!(f, "missing or invalid required frontmatter field '{field}'")
            }
            Self::NameMismatch {
                uri_name,
                frontmatter_name,
            } => write!(
                f,
                "skill URI name '{uri_name}' does not match frontmatter name '{frontmatter_name}'"
            ),
            Self::EmptyManifest => f.write_str("skill manifest must not be empty"),
            Self::MissingSkillMdInManifest => {
                f.write_str("skill manifest must include an entry for SKILL.md")
            }
            Self::DuplicateManifestUri(uri) => write!(f, "duplicate URI in skill manifest: {uri}"),
            Self::ManifestFileOutsideSkillRoot(uri) => {
                write!(f, "manifest file URI is outside skill root: {uri}")
            }
            Self::TooManyFiles { count, max } => {
                write!(f, "skill has {count} files, exceeding limit of {max}")
            }
            Self::TotalSizeExceeded { bytes, max } => {
                write!(f, "skill total size {bytes} bytes exceeds limit of {max}")
            }
            Self::UnlistedResource(uri) => {
                write!(
                    f,
                    "resource URI is not in the retained skill manifest: {uri}"
                )
            }
            Self::SizeMismatch { expected, actual } => {
                write!(
                    f,
                    "skill file size mismatch: expected {expected} bytes, got {actual}"
                )
            }
            Self::DigestMismatch { expected, actual } => {
                write!(
                    f,
                    "skill file digest mismatch: expected {expected}, got {actual}"
                )
            }
            Self::FrontmatterMismatch => {
                f.write_str("SKILL.md frontmatter does not match the retained skill entry")
            }
            Self::InvalidDirectoryUri(uri) => write!(f, "invalid skill directory URI: {uri}"),
            Self::DirectoryNotFound(uri) => write!(f, "skill directory not found: {uri}"),
            Self::DuplicateSkillUri(uri) => write!(f, "duplicate skill URI in catalog: {uri}"),
        }
    }
}

impl Error for McpSkillError {}
