#![deny(missing_docs)]

use super::ToolCallId;
use crate::domain::agent_execution::ExecutionError;

/// An untrusted path description, not a resolved file or permission to access it.
/// Relative and remote paths are valid; no host filesystem normalization occurs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePath(String);
impl FilePath {
    /// Preserve `value` exactly, rejecting an empty path or embedded NUL with
    /// [`ExecutionError::InvalidPath`]. Does not resolve, authorize, or access it.
    pub fn new(value: impl Into<String>) -> Result<Self, ExecutionError> {
        let value = value.into();
        if value.is_empty() || value.contains('\0') {
            return Err(ExecutionError::InvalidPath);
        }
        Ok(Self(value))
    }
    /// Borrow the original path spelling, including relative/remote syntax.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Provider-reported category used for display and review, not execution policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolKind {
    /// Inspects resource content.
    Read,
    /// Reports resource modification.
    Edit,
    /// Searches resources or content.
    Search,
    /// Retrieves remote content.
    Fetch,
    /// Runs a command or other executable activity.
    Execute,
    /// Plans or coordinates agent work.
    Think,
    /// Removes a resource.
    Delete,
    /// Moves or renames a resource.
    Move,
    /// Reports a provider mode transition.
    SwitchMode,
    /// A provider activity outside the recognized categories.
    Other,
}
/// Last provider-reported progress; completion does not independently verify effects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    /// Announced but not reported as executing.
    Pending,
    /// Reported as executing.
    Running,
    /// Provider reported successful completion.
    Completed,
    /// Provider reported failure; partial effects may remain.
    Failed,
}
/// An observed path and optional provider-reported line number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileLocation {
    path: FilePath,
    line: Option<u32>,
}
impl FileLocation {
    /// Retain `path` and optional `line` without filesystem lookup. Line numbering
    /// is supplied by the producer; this value does not normalize or reject zero.
    pub fn new(path: FilePath, line: Option<u32>) -> Self {
        Self { path, line }
    }
    /// Borrow the unresolved path reported by the provider.
    pub fn path(&self) -> &FilePath {
        &self.path
    }
    /// Return the reported line number, or None when no line was supplied.
    pub fn line(&self) -> Option<u32> {
        self.line
    }
}
/// Immutable sparse update addressed to a tool. None omits a field; Some(empty)
/// explicitly clears it. This is input to the entity, not its accumulated state.
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
    /// Own one sparse update for `id`. `title`, `kind`, and `status` replace their
    /// fields when present; `locations` and `content` replace whole collections.
    /// None leaves a field untouched; present empty strings/vectors clear it.
    /// No tool is executed and no observation is changed by construction.
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
    /// Tool identity to which this update must be applied.
    pub fn id(&self) -> &ToolCallId {
        &self.id
    }
    /// Replacement display title; None leaves the observed title untouched.
    pub fn title(&self) -> &Option<String> {
        &self.title
    }
    /// Replacement category; None retains the previous observation.
    pub fn kind(&self) -> &Option<ToolKind> {
        &self.kind
    }
    /// Replacement progress; None retains the previous observation.
    pub fn status(&self) -> &Option<ToolStatus> {
        &self.status
    }
    /// Replacement location collection; Some(empty) explicitly clears it.
    pub fn locations(&self) -> &Option<Vec<FileLocation>> {
        &self.locations
    }
    /// Replacement content collection; Some(empty) explicitly clears it.
    pub fn content(&self) -> &Option<Vec<ToolContent>> {
        &self.content
    }
    /// Retained title, path, and content allocations, including vector storage.
    /// ToolCall accounts for retained identities in addition to these observation bytes.
    pub fn payload_bytes(&self) -> usize {
        ToolObservation::measure_payload(&self.title, &self.locations, &self.content)
    }
}

/// Immutable snapshot of what has been observed about a tool, without identity.
/// None means the field has not been observed. ToolCall owns identity and replaces
/// this value when accepting an update; previously captured snapshots stay unchanged.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolObservation {
    title: Option<String>,
    kind: Option<ToolKind>,
    status: Option<ToolStatus>,
    locations: Option<Vec<FileLocation>>,
    content: Option<Vec<ToolContent>>,
}
impl ToolObservation {
    /// Last observed display title; None means no title has been observed.
    pub fn title(&self) -> &Option<String> {
        &self.title
    }
    /// Last observed category; None means it is still unknown.
    pub fn kind(&self) -> &Option<ToolKind> {
        &self.kind
    }
    /// Last observed progress; None means it is still unknown.
    pub fn status(&self) -> &Option<ToolStatus> {
        &self.status
    }
    /// Last observed locations; None is unknown, Some(empty) is an explicit empty list.
    pub fn locations(&self) -> &Option<Vec<FileLocation>> {
        &self.locations
    }
    /// Last observed content; None is unknown, Some(empty) is an explicit empty list.
    pub fn content(&self) -> &Option<Vec<ToolContent>> {
        &self.content
    }
    /// Retained payload allocation in bytes, including vector capacity. Saturates at
    /// usize::MAX on overflow; excludes fixed identity/entity storage.
    pub fn payload_bytes(&self) -> usize {
        Self::measure_payload(&self.title, &self.locations, &self.content)
    }
    pub(crate) fn payload_bytes_after(&self, update: &ToolCallUpdate) -> usize {
        Self::measure_payload(
            if update.title.is_some() {
                &update.title
            } else {
                &self.title
            },
            if update.locations.is_some() {
                &update.locations
            } else {
                &self.locations
            },
            if update.content.is_some() {
                &update.content
            } else {
                &self.content
            },
        )
    }
    fn measure_payload(
        title: &Option<String>,
        locations: &Option<Vec<FileLocation>>,
        content: &Option<Vec<ToolContent>>,
    ) -> usize {
        let mut bytes = title.as_ref().map_or(0, String::capacity);
        if let Some(locations) = locations {
            bytes = bytes.saturating_add(
                locations
                    .capacity()
                    .saturating_mul(size_of::<FileLocation>()),
            );
            for location in locations {
                bytes = bytes.saturating_add(location.path.0.capacity());
            }
        }
        if let Some(content) = content {
            bytes =
                bytes.saturating_add(content.capacity().saturating_mul(size_of::<ToolContent>()));
            for item in content {
                bytes = bytes.saturating_add(item.payload_bytes());
            }
        }
        bytes
    }
    /// Consume this snapshot and return a replacement using supplied `update` fields.
    /// Omitted fields retain their previous values; present empty fields clear them.
    /// Owned fields move without cloning. The value carries no tool or execution
    /// identity, so this operation does not validate correlation or update a session.
    /// Session-owned tool updates validate identities before constructing a replacement.
    /// Providers can use this to build a review snapshot from a sparse tool report.
    pub fn with_update(self, update: ToolCallUpdate) -> Self {
        Self {
            title: update.title.or(self.title),
            kind: update.kind.or(self.kind),
            status: update.status.or(self.status),
            locations: update.locations.or(self.locations),
            content: update.content.or(self.content),
        }
    }
}
/// Immutable provider-reported text or file change for observation and review.
/// Construction preserves exact text, including empty strings, and compacts text
/// allocations. This value never applies a change or authorizes filesystem access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolContent {
    value: ToolContentValue,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum ToolContentValue {
    Text(Box<str>),
    Diff {
        path: FilePath,
        old: Option<Box<str>>,
        new: Box<str>,
    },
}
/// Borrowed inspection of immutable tool content; all payload references are shared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolContentView<'a> {
    /// Exact observed text, including empty or whitespace-only text.
    Text(&'a str),
    /// A described file change; observing it does not apply or verify the change.
    Diff {
        /// Unresolved resource path supplied by the provider.
        path: &'a FilePath,
        /// Prior text when supplied; None differs from explicitly empty text.
        old: Option<&'a str>,
        /// Replacement text, which may be empty.
        new: &'a str,
    },
}
impl ToolContent {
    /// Own exact observed `text`, including empty text, without I/O or validation.
    /// Discards input spare capacity; creates no execution or permission authority.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            value: ToolContentValue::Text(text.into().into_boxed_str()),
        }
    }
    /// Describe a change to unresolved `path` with optional prior `old` text and
    /// replacement `new` text. None means prior text was not supplied; Some(empty)
    /// records explicitly empty prior text. Empty replacement text is valid.
    /// Compacts both text allocations without applying or authorizing the change.
    pub fn diff(path: FilePath, old: Option<String>, new: impl Into<String>) -> Self {
        Self {
            value: ToolContentValue::Diff {
                path,
                old: old.map(String::into_boxed_str),
                new: new.into().into_boxed_str(),
            },
        }
    }
    /// Inspect the content through shared references, without copying or mutation.
    /// Borrowed text cannot expose an in-place edit of this value:
    ///
    /// ```compile_fail
    /// use nessa_sdk::domain::agent_execution::tools::{ToolContent, ToolContentView};
    /// let content = ToolContent::text("original");
    /// if let ToolContentView::Text(text) = content.view() {
    ///     text.push_str(" changed");
    /// }
    /// ```
    pub fn view(&self) -> ToolContentView<'_> {
        match &self.value {
            ToolContentValue::Text(text) => ToolContentView::Text(text),
            ToolContentValue::Diff { path, old, new } => ToolContentView::Diff {
                path,
                old: old.as_deref(),
                new,
            },
        }
    }
    /// Retained variable payload bytes: compact text lengths and the path's actual
    /// allocation capacity. Collection slots are measured by the owning observation.
    pub fn payload_bytes(&self) -> usize {
        match &self.value {
            ToolContentValue::Text(text) => text.len(),
            ToolContentValue::Diff { path, old, new } => path
                .0
                .capacity()
                .saturating_add(old.as_ref().map_or(0, |text| text.len()))
                .saturating_add(new.len()),
        }
    }
}
