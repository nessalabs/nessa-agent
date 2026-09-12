use super::ToolCallId;
use crate::domain::agent_execution::ExecutionError;

/// An untrusted path description, not a resolved file or permission to access it.
/// Relative and remote paths are valid; no host filesystem normalization occurs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePath(String);
impl FilePath {
    pub fn new(value: impl Into<String>) -> Result<Self, ExecutionError> {
        let value = value.into();
        if value.is_empty() || value.contains('\0') {
            return Err(ExecutionError::InvalidPath);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolKind {
    Read,
    Edit,
    Search,
    Other,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    Pending,
    Running,
    Completed,
    Failed,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileLocation {
    path: FilePath,
    line: Option<u32>,
}
impl FileLocation {
    pub fn new(path: FilePath, line: Option<u32>) -> Self {
        Self { path, line }
    }
    pub fn path(&self) -> &FilePath {
        &self.path
    }
    pub fn line(&self) -> Option<u32> {
        self.line
    }
}
/// Sparse observation: None leaves existing data intact; Some(empty) clears it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallUpdate {
    id: ToolCallId,
    title: Option<String>,
    kind: Option<ToolKind>,
    status: Option<ToolStatus>,
    locations: Option<Vec<FileLocation>>,
    content: Option<Vec<ToolContent>>,
}
impl ToolCallUpdate {
    pub fn new(
        id: ToolCallId,
        title: Option<String>,
        kind: Option<ToolKind>,
        status: Option<ToolStatus>,
        locations: Option<Vec<FileLocation>>,
        content: Option<Vec<ToolContent>>,
    ) -> Self {
        Self {
            id,
            title,
            kind,
            status,
            locations,
            content,
        }
    }
    pub fn id(&self) -> &ToolCallId {
        &self.id
    }
    pub fn title(&self) -> &Option<String> {
        &self.title
    }
    pub fn kind(&self) -> &Option<ToolKind> {
        &self.kind
    }
    pub fn status(&self) -> &Option<ToolStatus> {
        &self.status
    }
    pub fn locations(&self) -> &Option<Vec<FileLocation>> {
        &self.locations
    }
    pub fn content(&self) -> &Option<Vec<ToolContent>> {
        &self.content
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolContent {
    Text(String),
    Diff {
        path: FilePath,
        old: Option<String>,
        new: String,
    },
}

/// Normalized file-tool inputs for a host to review before allowing an action.
/// Paths and contents are untrusted provider data, never authorization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileToolInput {
    Read {
        path: FilePath,
        offset: Option<u64>,
        limit: Option<u64>,
        pages: Option<String>,
    },
    Write {
        path: FilePath,
        content: String,
    },
    Edit {
        path: FilePath,
        old: String,
        new: String,
        replace_all: bool,
    },
    Glob {
        pattern: String,
        path: Option<FilePath>,
    },
    Grep {
        pattern: String,
        path: Option<FilePath>,
        glob: Option<String>,
        file_type: Option<String>,
        output_mode: Option<String>,
        before: Option<u64>,
        after: Option<u64>,
        context: Option<u64>,
        line_numbers: Option<bool>,
        ignore_case: Option<bool>,
        only_matching: Option<bool>,
        multiline: Option<bool>,
        head_limit: Option<u64>,
        offset: Option<u64>,
    },
}
