//! What an MCP server publishes for the Skills extension (SEP-2640,
//! `io.modelcontextprotocol/skills`): validated skill entries, YAML frontmatter,
//! SHA-256 file manifests, content verification, and static skill catalogs.
//!
//! ```text
//! SkillCatalog --> SkillEntry --> SkillUri --> SkillName
//!                       |------> SkillFrontmatter --> FrontmatterValue
//!                       `------> SkillResources (Manifest | Dynamic)
//!                                     `--> SkillResourceEntry --> SkillFileUri + SkillDigest
//! ```
//!
//! Arrows mean "holds". All invariants are enforced on construction: skill names
//! satisfy Agent Skills naming rules, `SKILL.md` URIs end in `/<name>/SKILL.md`
//! matching `frontmatter.name`, manifests stay within SEP-2640's 512-file and
//! 16 MiB bounds, and `SkillEntry::verify_file` refuses any unlisted URI, size
//! mismatch, SHA-256 digest mismatch, or frontmatter discrepancy.
#![deny(missing_docs)]

mod catalog;
mod error;
mod frontmatter;
mod value_objects;

/// The identifier of the MCP Skills extension in `capabilities.extensions`.
pub const EXTENSION: &str = "io.modelcontextprotocol/skills";
/// The conventional URI scheme for MCP-served skills.
pub const SKILL_URI_SCHEME: &str = "skill";
/// The required root instruction file name for every Agent Skill.
pub const SKILL_FILE_NAME: &str = "SKILL.md";
/// The MIME type used for child directories in `resources/directory/read`.
pub const DIRECTORY_MIME_TYPE: &str = "inode/directory";
/// SEP-2640 per-skill limit on resource manifest entries, including `SKILL.md`.
pub const MAX_FILES_PER_SKILL: usize = 512;
/// SEP-2640 per-skill limit on total raw bytes across all files (16 MiB).
pub const MAX_SKILL_TOTAL_BYTES: u64 = 16 * 1024 * 1024;
/// Maximum byte length of a skill or file URI.
pub const MAX_SKILL_URI_BYTES: usize = 2048;
/// Maximum byte length of an Agent Skill name.
pub const MAX_SKILL_NAME_BYTES: usize = 64;

pub use catalog::{SkillCatalog, SkillDefinition, SkillFile};
pub use error::McpSkillError;
pub use frontmatter::parse_frontmatter;
pub use value_objects::{
    DirectoryChild, FrontmatterValue, SkillDigest, SkillEntry, SkillFileUri, SkillFrontmatter,
    SkillName, SkillResourceEntry, SkillResources, SkillUri, SkillsCapability,
};
