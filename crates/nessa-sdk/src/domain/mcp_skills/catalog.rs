use super::{
    parse_frontmatter, DirectoryChild, McpSkillError, SkillDigest, SkillEntry, SkillResourceEntry,
    SkillResources, SkillUri, DIRECTORY_MIME_TYPE, SKILL_FILE_NAME, SKILL_URI_SCHEME,
};
use std::collections::{BTreeMap, BTreeSet};

/// One file belonging to a statically served skill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillFile {
    /// Path relative to the skill root (for example `SKILL.md` or
    /// `references/workflows.md`).
    pub relative_path: String,
    /// MIME type advertised on `resources/read` and `resources/list`.
    pub mime_type: String,
    /// Raw file bytes (digests and sizes are computed over these exact bytes).
    pub bytes: Vec<u8>,
}

impl SkillFile {
    /// Construct a UTF-8 Markdown file.
    pub fn markdown(relative_path: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            relative_path: relative_path.into(),
            mime_type: "text/markdown".to_owned(),
            bytes: content.into().into_bytes(),
        }
    }
}

/// Input definition for registering one skill into a [`SkillCatalog`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillDefinition {
    /// `/`-separated skill path (final segment must match `frontmatter.name`),
    /// served under `skill://<skill_path>/...`.
    pub skill_path: String,
    /// All files in the skill directory, including `SKILL.md`.
    pub files: Vec<SkillFile>,
}

#[derive(Clone, Debug)]
struct StoredResource {
    uri: String,
    name: String,
    description: Option<String>,
    mime_type: String,
    bytes: Vec<u8>,
}

/// Immutable, validated catalog of Agent Skills served by an MCP server.
///
/// Built once from embedded or static skill definitions; computes SHA-256
/// digests, byte sizes, parsed frontmatter, and directory listings up front so
/// `skills/list`, `skills/get`, `resources/list`, `resources/read`, and
/// `resources/directory/read` share one authoritative source of truth.
#[derive(Clone, Debug)]
pub struct SkillCatalog {
    entries: Vec<SkillEntry>,
    by_uri: BTreeMap<String, usize>,
    resources: BTreeMap<String, StoredResource>,
    directories: BTreeMap<String, Vec<DirectoryChild>>,
    digest_inputs: Vec<String>,
}

impl SkillCatalog {
    /// Validate and build a [`SkillCatalog`] from one or more [`SkillDefinition`]s.
    pub fn new(definitions: Vec<SkillDefinition>) -> Result<Self, McpSkillError> {
        let mut entries = Vec::with_capacity(definitions.len());
        let mut by_uri = BTreeMap::new();
        let mut resources = BTreeMap::new();
        let mut dir_sets: BTreeMap<String, BTreeMap<String, DirectoryChild>> = BTreeMap::new();
        let mut digest_inputs = BTreeSet::new();

        for def in definitions {
            let skill_uri = SkillUri::new(format!(
                "{SKILL_URI_SCHEME}://{}/{SKILL_FILE_NAME}",
                def.skill_path
            ))?;
            if by_uri.contains_key(skill_uri.as_str()) {
                return Err(McpSkillError::DuplicateSkillUri(
                    skill_uri.as_str().to_owned(),
                ));
            }
            let Some(skill_md_file) = def
                .files
                .iter()
                .find(|file| file.relative_path == SKILL_FILE_NAME)
            else {
                return Err(McpSkillError::MissingSkillMdInManifest);
            };
            let skill_md_text = std::str::from_utf8(&skill_md_file.bytes).map_err(|_| {
                McpSkillError::InvalidFrontmatter("SKILL.md is not valid UTF-8".to_owned())
            })?;
            let frontmatter = parse_frontmatter(skill_md_text)?;

            let mut manifest = Vec::with_capacity(def.files.len());
            for file in def.files {
                let file_uri = skill_uri.resolve_relative(&file.relative_path)?;
                let digest = SkillDigest::from_bytes(&file.bytes);
                let size = file.bytes.len() as u64;
                digest_inputs.insert(format!(
                    "skill:{}:{}:{}",
                    skill_uri.as_str(),
                    file_uri.as_str(),
                    digest.as_str()
                ));
                manifest.push(SkillResourceEntry::new(file_uri.clone(), digest, size));

                let (name, description) = if file.relative_path == SKILL_FILE_NAME {
                    (
                        frontmatter.name().as_str().to_owned(),
                        Some(frontmatter.description().to_owned()),
                    )
                } else {
                    let basename = file
                        .relative_path
                        .rsplit('/')
                        .next()
                        .unwrap_or(&file.relative_path)
                        .to_owned();
                    (basename, None)
                };
                register_directory_ancestors(
                    &mut dir_sets,
                    skill_uri.root_dir(),
                    &file.relative_path,
                    file_uri.as_str(),
                    &file.mime_type,
                );
                resources.insert(
                    file_uri.as_str().to_owned(),
                    StoredResource {
                        uri: file_uri.as_str().to_owned(),
                        name,
                        description,
                        mime_type: file.mime_type,
                        bytes: file.bytes,
                    },
                );
            }

            let entry = SkillEntry::new(
                skill_uri.clone(),
                frontmatter,
                SkillResources::Manifest(manifest),
            )?;
            let idx = entries.len();
            by_uri.insert(skill_uri.as_str().to_owned(), idx);
            entries.push(entry);
        }

        let directories = dir_sets
            .into_iter()
            .map(|(dir, children)| (dir, children.into_values().collect()))
            .collect();

        Ok(Self {
            entries,
            by_uri,
            resources,
            directories,
            digest_inputs: digest_inputs.into_iter().collect(),
        })
    }

    /// Whether this catalog serves any skills.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// All skill entries in deterministic registration order.
    pub fn entries(&self) -> &[SkillEntry] {
        &self.entries
    }

    /// Look up a single skill entry by its `SKILL.md` URI.
    pub fn get(&self, uri: &str) -> Option<&SkillEntry> {
        let idx = *self.by_uri.get(uri)?;
        self.entries.get(idx)
    }

    /// Read a skill resource by URI, returning `(uri, mime_type, raw_bytes)`.
    pub fn read_resource(&self, uri: &str) -> Option<(&str, &str, &[u8])> {
        let res = self.resources.get(uri)?;
        Some((&res.uri, &res.mime_type, &res.bytes))
    }

    /// List all resources served by this catalog as `(uri, name, description, mime_type)`.
    pub fn list_resources(&self) -> Vec<(&str, &str, Option<&str>, &str)> {
        self.resources
            .values()
            .map(|res| {
                (
                    res.uri.as_str(),
                    res.name.as_str(),
                    res.description.as_deref(),
                    res.mime_type.as_str(),
                )
            })
            .collect()
    }

    /// Read direct children of a skill directory URI (no trailing slash).
    pub fn read_directory(&self, dir_uri: &str) -> Result<&[DirectoryChild], McpSkillError> {
        if dir_uri.ends_with('/') {
            return Err(McpSkillError::InvalidDirectoryUri(dir_uri.to_owned()));
        }
        self.directories
            .get(dir_uri)
            .map(Vec::as_slice)
            .ok_or_else(|| McpSkillError::DirectoryNotFound(dir_uri.to_owned()))
    }

    /// Stable digest inputs for configuration hashing (for example,
    /// `BuiltInServer::configuration`): changes whenever any skill URI,
    /// resource URI, or file digest changes.
    pub fn digest_inputs(&self) -> Vec<String> {
        self.digest_inputs.clone()
    }
}

fn register_directory_ancestors(
    dir_sets: &mut BTreeMap<String, BTreeMap<String, DirectoryChild>>,
    root_dir: &str,
    relative_path: &str,
    file_uri: &str,
    file_mime: &str,
) {
    let segments: Vec<&str> = relative_path.split('/').collect();
    let mut current_dir = root_dir.to_owned();
    for (idx, seg) in segments.iter().enumerate() {
        let is_last = idx + 1 == segments.len();
        if is_last {
            dir_sets.entry(current_dir.clone()).or_default().insert(
                (*seg).to_owned(),
                DirectoryChild {
                    uri: file_uri.to_owned(),
                    name: (*seg).to_owned(),
                    mime_type: file_mime.to_owned(),
                },
            );
        } else {
            let child_dir = format!("{current_dir}/{seg}");
            dir_sets.entry(current_dir.clone()).or_default().insert(
                (*seg).to_owned(),
                DirectoryChild {
                    uri: child_dir.clone(),
                    name: (*seg).to_owned(),
                    mime_type: DIRECTORY_MIME_TYPE.to_owned(),
                },
            );
            current_dir = child_dir;
        }
    }
}
