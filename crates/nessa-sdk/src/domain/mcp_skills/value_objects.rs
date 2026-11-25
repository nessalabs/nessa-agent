use super::{
    parse_frontmatter, McpSkillError, DIRECTORY_MIME_TYPE, MAX_FILES_PER_SKILL,
    MAX_SKILL_NAME_BYTES, MAX_SKILL_TOTAL_BYTES, MAX_SKILL_URI_BYTES, SKILL_FILE_NAME,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// Server-advertised capabilities for `io.modelcontextprotocol/skills`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SkillsCapability {
    /// Whether the server declared `"directoryRead": true` and therefore
    /// supports `resources/directory/read`.
    pub directory_read: bool,
}

/// Validated Agent Skill name (1..=64 lowercase ASCII letters, digits, or `-`,
/// no leading/trailing `-`, no consecutive `--`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillName(String);

impl SkillName {
    /// Validate and construct a [`SkillName`].
    pub fn new(raw: impl Into<String>) -> Result<Self, McpSkillError> {
        let value = raw.into();
        if value.is_empty()
            || value.len() > MAX_SKILL_NAME_BYTES
            || value.starts_with('-')
            || value.ends_with('-')
            || value.contains("--")
            || !value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(McpSkillError::InvalidSkillName);
        }
        Ok(Self(value))
    }

    /// Borrow the validated skill name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Validated resource URI for a skill's `SKILL.md` (e.g. `skill://code-review/SKILL.md`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillUri {
    uri: String,
    root_dir: String,
    skill_name: SkillName,
}

impl SkillUri {
    /// Validate and construct a [`SkillUri`].
    ///
    /// Accepts any valid URI scheme (conventionally `skill://`) whose path ends
    /// in `/<skill-name>/SKILL.md` with no query or fragment.
    pub fn new(raw: impl Into<String>) -> Result<Self, McpSkillError> {
        let uri = raw.into();
        validate_uri_syntax(&uri, McpSkillError::InvalidSkillUri)?;
        let Some(root_dir) = uri.strip_suffix(&format!("/{SKILL_FILE_NAME}")) else {
            return Err(McpSkillError::InvalidSkillUri);
        };
        let Some(scheme_sep) = root_dir.find("://") else {
            return Err(McpSkillError::InvalidSkillUri);
        };
        let after_scheme = &root_dir[scheme_sep + 3..];
        if after_scheme.is_empty() || after_scheme.ends_with('/') {
            return Err(McpSkillError::InvalidSkillUri);
        }
        let segments: Vec<&str> = after_scheme.split('/').collect();
        if segments
            .iter()
            .any(|seg| seg.is_empty() || *seg == "." || *seg == "..")
        {
            return Err(McpSkillError::InvalidSkillUri);
        }
        let last = *segments.last().ok_or(McpSkillError::InvalidSkillUri)?;
        let skill_name = SkillName::new(last).map_err(|_| McpSkillError::InvalidSkillUri)?;
        Ok(Self {
            uri: uri.clone(),
            root_dir: root_dir.to_owned(),
            skill_name,
        })
    }

    /// Full URI of the skill's `SKILL.md`.
    pub fn as_str(&self) -> &str {
        &self.uri
    }

    /// Root directory URI of the skill (without `/SKILL.md` and without a trailing slash).
    pub fn root_dir(&self) -> &str {
        &self.root_dir
    }

    /// Skill name derived from the final segment of `<skill-path>`.
    pub fn skill_name(&self) -> &SkillName {
        &self.skill_name
    }

    /// Resolve a relative file path (such as `references/checklist.md`) against
    /// this skill's root directory, refusing path traversal (`..`), leading `/`,
    /// empty segments, or backslashes.
    pub fn resolve_relative(&self, relative: &str) -> Result<SkillFileUri, McpSkillError> {
        if relative.is_empty()
            || relative.starts_with('/')
            || relative.ends_with('/')
            || relative.contains('\\')
            || relative.contains('?')
            || relative.contains('#')
            || relative.bytes().any(|b| b.is_ascii_control() || b == b' ')
        {
            return Err(McpSkillError::InvalidRelativePath);
        }
        let mut normalized = Vec::new();
        for seg in relative.split('/') {
            if seg.is_empty() || seg == "." || seg == ".." {
                return Err(McpSkillError::InvalidRelativePath);
            }
            normalized.push(seg);
        }
        let joined = format!("{}/{}", self.root_dir, normalized.join("/"));
        SkillFileUri::new(joined)
    }
}

/// Validated resource URI of a file inside a skill directory.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillFileUri(String);

impl SkillFileUri {
    /// Validate and construct a [`SkillFileUri`].
    pub fn new(raw: impl Into<String>) -> Result<Self, McpSkillError> {
        let uri = raw.into();
        validate_uri_syntax(&uri, McpSkillError::InvalidFileUri)?;
        if uri.ends_with('/') {
            return Err(McpSkillError::InvalidFileUri);
        }
        Ok(Self(uri))
    }

    /// Borrow the file URI string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Relative path beneath `root_dir`, if this file sits strictly inside it.
    pub fn relative_to<'a>(&'a self, root_dir: &str) -> Option<&'a str> {
        let rest = self.0.strip_prefix(root_dir)?;
        let rel = rest.strip_prefix('/')?;
        if rel.is_empty()
            || rel
                .split('/')
                .any(|seg| seg.is_empty() || seg == "." || seg == "..")
        {
            return None;
        }
        Some(rel)
    }
}

impl From<&SkillUri> for SkillFileUri {
    fn from(uri: &SkillUri) -> Self {
        Self(uri.uri.clone())
    }
}

/// Validated `sha256:{64 lowercase hex}` digest for a skill file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SkillDigest(String);

impl SkillDigest {
    /// Compute the SEP-2640 SHA-256 digest of raw file bytes.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let hash = Sha256::digest(bytes);
        let mut out = String::with_capacity(7 + 64);
        out.push_str("sha256:");
        for byte in hash {
            out.push(hex_nibble(byte >> 4));
            out.push(hex_nibble(byte & 0x0f));
        }
        Self(out)
    }

    /// Validate and parse a `sha256:{64 lowercase hex}` digest string.
    pub fn parse(raw: impl Into<String>) -> Result<Self, McpSkillError> {
        let value = raw.into();
        let Some(hex) = value.strip_prefix("sha256:") else {
            return Err(McpSkillError::InvalidDigest);
        };
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(McpSkillError::InvalidDigest);
        }
        Ok(Self(value))
    }

    /// Borrow the digest string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Verify that `bytes` match this digest.
    pub fn verify(&self, bytes: &[u8]) -> Result<(), McpSkillError> {
        let actual = Self::from_bytes(bytes);
        if actual != *self {
            return Err(McpSkillError::DigestMismatch {
                expected: self.0.clone(),
                actual: actual.0,
            });
        }
        Ok(())
    }
}

/// Pure representation of a YAML frontmatter value for exact field-by-field
/// comparison without transport-layer dependencies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrontmatterValue {
    /// YAML `null` / `~`.
    Null,
    /// YAML boolean.
    Bool(bool),
    /// YAML integer.
    Integer(i64),
    /// YAML string scalar.
    String(String),
    /// YAML sequence.
    Sequence(Vec<FrontmatterValue>),
    /// YAML mapping.
    Mapping(BTreeMap<String, FrontmatterValue>),
}

/// Verbatim YAML frontmatter of a `SKILL.md` file, validated to contain `name`
/// and non-empty `description`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillFrontmatter {
    name: SkillName,
    description: String,
    fields: BTreeMap<String, FrontmatterValue>,
}

impl SkillFrontmatter {
    /// Validate and construct [`SkillFrontmatter`] from its verbatim fields.
    pub fn new(fields: BTreeMap<String, FrontmatterValue>) -> Result<Self, McpSkillError> {
        let name_str = match fields.get("name") {
            Some(FrontmatterValue::String(s)) => s.clone(),
            _ => return Err(McpSkillError::MissingFrontmatterField("name")),
        };
        let name = SkillName::new(name_str)?;
        let description = match fields.get("description") {
            Some(FrontmatterValue::String(s)) if !s.trim().is_empty() => s.clone(),
            _ => return Err(McpSkillError::MissingFrontmatterField("description")),
        };
        Ok(Self {
            name,
            description,
            fields,
        })
    }

    /// Required `name` field.
    pub fn name(&self) -> &SkillName {
        &self.name
    }

    /// Required `description` field.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Complete verbatim frontmatter fields.
    pub fn fields(&self) -> &BTreeMap<String, FrontmatterValue> {
        &self.fields
    }
}

/// One file entry inside a skill's manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillResourceEntry {
    uri: SkillFileUri,
    digest: SkillDigest,
    size: u64,
}

impl SkillResourceEntry {
    /// Construct a manifest resource entry.
    pub fn new(uri: SkillFileUri, digest: SkillDigest, size: u64) -> Self {
        Self { uri, digest, size }
    }

    /// File URI.
    pub fn uri(&self) -> &SkillFileUri {
        &self.uri
    }

    /// Declared SHA-256 digest.
    pub fn digest(&self) -> &SkillDigest {
        &self.digest
    }

    /// Declared byte length.
    pub fn size(&self) -> u64 {
        self.size
    }
}

/// Complete file manifest or `"dynamic"` marker on a [`SkillEntry`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkillResources {
    /// Complete manifest of all files in the skill, validated against SEP-2640 limits.
    Manifest(Vec<SkillResourceEntry>),
    /// Explicitly dynamic skill whose content is generated without stable digests.
    Dynamic,
}

/// One direct child returned by `resources/directory/read`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryChild {
    /// Child resource or directory URI (no trailing slash for directories).
    pub uri: String,
    /// Basename of the child.
    pub name: String,
    /// MIME type (`inode/directory` for subdirectories).
    pub mime_type: String,
}

/// Validated SEP-2640 skill entry as returned by `skills/list` or `skills/get`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillEntry {
    uri: SkillUri,
    frontmatter: SkillFrontmatter,
    resources: SkillResources,
}

impl SkillEntry {
    /// Validate and construct a [`SkillEntry`], enforcing URI/name agreement,
    /// manifest completeness, containment within the skill root, and SEP-2640
    /// file count (512) and total byte size (16 MiB) limits.
    pub fn new(
        uri: SkillUri,
        frontmatter: SkillFrontmatter,
        resources: SkillResources,
    ) -> Result<Self, McpSkillError> {
        if uri.skill_name() != frontmatter.name() {
            return Err(McpSkillError::NameMismatch {
                uri_name: uri.skill_name().as_str().to_owned(),
                frontmatter_name: frontmatter.name().as_str().to_owned(),
            });
        }
        if let SkillResources::Manifest(ref entries) = resources {
            if entries.is_empty() {
                return Err(McpSkillError::EmptyManifest);
            }
            if entries.len() > MAX_FILES_PER_SKILL {
                return Err(McpSkillError::TooManyFiles {
                    count: entries.len(),
                    max: MAX_FILES_PER_SKILL,
                });
            }
            let mut seen = HashSet::with_capacity(entries.len());
            let mut has_skill_md = false;
            let mut total_bytes: u64 = 0;
            for entry in entries {
                let file_uri = entry.uri().as_str();
                if !seen.insert(file_uri) {
                    return Err(McpSkillError::DuplicateManifestUri(file_uri.to_owned()));
                }
                if entry.uri().relative_to(uri.root_dir()).is_none() {
                    return Err(McpSkillError::ManifestFileOutsideSkillRoot(
                        file_uri.to_owned(),
                    ));
                }
                if file_uri == uri.as_str() {
                    has_skill_md = true;
                }
                total_bytes = total_bytes.checked_add(entry.size()).ok_or(
                    McpSkillError::TotalSizeExceeded {
                        bytes: u64::MAX,
                        max: MAX_SKILL_TOTAL_BYTES,
                    },
                )?;
                if total_bytes > MAX_SKILL_TOTAL_BYTES {
                    return Err(McpSkillError::TotalSizeExceeded {
                        bytes: total_bytes,
                        max: MAX_SKILL_TOTAL_BYTES,
                    });
                }
            }
            if !has_skill_md {
                return Err(McpSkillError::MissingSkillMdInManifest);
            }
        }
        Ok(Self {
            uri,
            frontmatter,
            resources,
        })
    }

    /// Skill's `SKILL.md` URI.
    pub fn uri(&self) -> &SkillUri {
        &self.uri
    }

    /// Verbatim YAML frontmatter from the entry.
    pub fn frontmatter(&self) -> &SkillFrontmatter {
        &self.frontmatter
    }

    /// File manifest or `"dynamic"` marker.
    pub fn resources(&self) -> &SkillResources {
        &self.resources
    }

    /// Verify raw bytes retrieved for `file_uri` against this retained entry:
    /// - For manifest skills: `file_uri` must be in the manifest, exact byte
    ///   size must match, and SHA-256 digest must match.
    /// - For `SKILL.md` (both manifest and dynamic): parsed YAML frontmatter
    ///   must equal `self.frontmatter` field-by-field.
    pub fn verify_file(&self, file_uri: &SkillFileUri, bytes: &[u8]) -> Result<(), McpSkillError> {
        match &self.resources {
            SkillResources::Manifest(entries) => {
                let Some(manifest_entry) = entries.iter().find(|e| e.uri() == file_uri) else {
                    return Err(McpSkillError::UnlistedResource(
                        file_uri.as_str().to_owned(),
                    ));
                };
                let actual_size = bytes.len() as u64;
                if actual_size != manifest_entry.size() {
                    return Err(McpSkillError::SizeMismatch {
                        expected: manifest_entry.size(),
                        actual: actual_size,
                    });
                }
                manifest_entry.digest().verify(bytes)?;
            }
            SkillResources::Dynamic => {
                if file_uri.relative_to(self.uri.root_dir()).is_none() {
                    return Err(McpSkillError::UnlistedResource(
                        file_uri.as_str().to_owned(),
                    ));
                }
            }
        }
        if file_uri.as_str() == self.uri.as_str() {
            let text = std::str::from_utf8(bytes).map_err(|_| {
                McpSkillError::InvalidFrontmatter("SKILL.md is not valid UTF-8".to_owned())
            })?;
            let parsed = parse_frontmatter(text)?;
            if parsed != self.frontmatter {
                return Err(McpSkillError::FrontmatterMismatch);
            }
        }
        Ok(())
    }

    /// Compute direct children of `dir_uri` from a manifest entry.
    pub fn directory_children(&self, dir_uri: &str) -> Result<Vec<DirectoryChild>, McpSkillError> {
        if dir_uri.ends_with('/') {
            return Err(McpSkillError::InvalidDirectoryUri(dir_uri.to_owned()));
        }
        let SkillResources::Manifest(entries) = &self.resources else {
            return Err(McpSkillError::DirectoryNotFound(dir_uri.to_owned()));
        };
        let prefix = format!("{dir_uri}/");
        let mut dirs = BTreeSet::new();
        let mut files = BTreeMap::new();
        for entry in entries {
            let Some(rest) = entry.uri().as_str().strip_prefix(&prefix) else {
                continue;
            };
            if let Some((subdir, _)) = rest.split_once('/') {
                dirs.insert(subdir.to_owned());
            } else if !rest.is_empty() {
                files.insert(rest.to_owned(), entry.uri().as_str().to_owned());
            }
        }
        if dirs.is_empty() && files.is_empty() {
            return Err(McpSkillError::DirectoryNotFound(dir_uri.to_owned()));
        }
        let mut out = Vec::with_capacity(dirs.len() + files.len());
        for subdir in dirs {
            out.push(DirectoryChild {
                uri: format!("{dir_uri}/{subdir}"),
                name: subdir,
                mime_type: DIRECTORY_MIME_TYPE.to_owned(),
            });
        }
        for (name, uri) in files {
            out.push(DirectoryChild {
                uri,
                mime_type: guess_mime_type(&name).to_owned(),
                name,
            });
        }
        Ok(out)
    }
}

pub(crate) fn guess_mime_type(name: &str) -> &'static str {
    if name.ends_with(".md") {
        "text/markdown"
    } else if name.ends_with(".json") {
        "application/json"
    } else if name.ends_with(".yaml") || name.ends_with(".yml") {
        "application/yaml"
    } else if name.ends_with(".txt") {
        "text/plain"
    } else {
        "application/octet-stream"
    }
}

fn validate_uri_syntax(uri: &str, err: McpSkillError) -> Result<(), McpSkillError> {
    if uri.is_empty() || uri.len() > MAX_SKILL_URI_BYTES {
        return Err(err);
    }
    if uri
        .bytes()
        .any(|b| b.is_ascii_control() || b.is_ascii_whitespace() || b == b'?' || b == b'#')
    {
        return Err(err);
    }
    let Some(sep) = uri.find("://") else {
        return Err(err);
    };
    let scheme = &uri[..sep];
    if scheme.is_empty()
        || !scheme.as_bytes()[0].is_ascii_alphabetic()
        || !scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
    {
        return Err(err);
    }
    Ok(())
}

fn hex_nibble(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        10..=15 => (b'a' + (nibble - 10)) as char,
        _ => unreachable!(),
    }
}
