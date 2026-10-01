#![deny(missing_docs)]

use super::{json::is_json, McpTool, ToolCallId};
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
    // Boxed: most tools are not MCP calls, and an observation is held (and
    // undone) inline in larger values.
    mcp_tool: Option<Box<McpTool>>,
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
            mcp_tool: None,
        }
    }
    /// This update with `content` as its replacement content collection, which
    /// replaces any the update carried. Returns a replacement; the original
    /// value is consumed, never changed in place.
    pub fn with_content(self, content: Vec<ToolContent>) -> Self {
        Self {
            content: Some(content),
            ..self
        }
    }
    /// This update, also naming the MCP server and tool the call was made to.
    /// Returns a replacement; the original value is consumed, never changed in
    /// place. Without it the update leaves any identity already observed as it
    /// was: an MCP identity is observed once and not cleared.
    pub fn with_mcp_tool(self, mcp_tool: McpTool) -> Self {
        Self {
            mcp_tool: Some(Box::new(mcp_tool)),
            ..self
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
    /// The MCP server and tool this update names; None leaves the observed one.
    pub fn mcp_tool(&self) -> Option<&McpTool> {
        self.mcp_tool.as_deref()
    }
    /// Retained title, path, content and MCP identity allocations, including
    /// vector and box storage. ToolCall accounts for retained identities in addition to
    /// these observation bytes.
    pub fn payload_bytes(&self) -> usize {
        ToolObservation::measure_payload(
            &self.title,
            &self.locations,
            &self.content,
            &self.mcp_tool,
        )
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
    // Boxed: most tools are not MCP calls, and an observation is held (and
    // undone) inline in larger values.
    mcp_tool: Option<Box<McpTool>>,
}
/// Moved prior fields for a reversible entity-owned observation replacement.
pub(crate) struct ToolObservationUndo {
    previous: ToolObservation,
    changed: [bool; 6],
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
    /// The MCP server and tool the call was made to; None where no update has
    /// named one — a harness's own tool, or a harness that does not say.
    pub fn mcp_tool(&self) -> Option<&McpTool> {
        self.mcp_tool.as_deref()
    }
    /// Retained payload allocation in bytes, including vector capacity. Saturates at
    /// usize::MAX on overflow; excludes fixed identity/entity storage.
    pub fn payload_bytes(&self) -> usize {
        Self::measure_payload(&self.title, &self.locations, &self.content, &self.mcp_tool)
    }
    /// A complete update for `id` that rebuilds this observation from nothing:
    /// every observed field present, so applying it to an empty observation
    /// yields an equal one. Clones the payload.
    pub(crate) fn as_update(&self, id: ToolCallId) -> ToolCallUpdate {
        ToolCallUpdate {
            id,
            title: self.title.clone(),
            kind: self.kind,
            status: self.status,
            locations: self.locations.clone(),
            content: self.content.clone(),
            mcp_tool: self.mcp_tool.clone(),
        }
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
            if update.mcp_tool.is_some() {
                &update.mcp_tool
            } else {
                &self.mcp_tool
            },
        )
    }
    fn measure_payload(
        title: &Option<String>,
        locations: &Option<Vec<FileLocation>>,
        content: &Option<Vec<ToolContent>>,
        mcp_tool: &Option<Box<McpTool>>,
    ) -> usize {
        let mut bytes = title.as_ref().map_or(0, String::capacity);
        bytes = bytes.saturating_add(mcp_tool.as_ref().map_or(0, |tool| {
            size_of::<McpTool>().saturating_add(tool.payload_bytes())
        }));
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
    /// identity, so this operation does not validate correlation or update a session;
    /// a different MCP identity replaces the earlier one here, and only the session's
    /// tool entity refuses that (`ExecutionError::DifferentMcpTool`).
    /// Session-owned tool updates validate identities before constructing a replacement.
    /// Providers can use this to build a review snapshot from a sparse tool report.
    pub fn with_update(self, update: ToolCallUpdate) -> Self {
        self.with_reversible_update(update).0
    }
    pub(crate) fn with_reversible_update(
        self,
        update: ToolCallUpdate,
    ) -> (Self, ToolObservationUndo) {
        let changed = [
            update.title.is_some(),
            update.kind.is_some(),
            update.status.is_some(),
            update.locations.is_some(),
            update.content.is_some(),
            update.mcp_tool.is_some(),
        ];
        let mut previous = self;
        let next = Self {
            title: update.title.or_else(|| previous.title.take()),
            kind: update.kind.or_else(|| previous.kind.take()),
            status: update.status.or_else(|| previous.status.take()),
            locations: update.locations.or_else(|| previous.locations.take()),
            content: update.content.or_else(|| previous.content.take()),
            mcp_tool: update.mcp_tool.or_else(|| previous.mcp_tool.take()),
        };
        (next, ToolObservationUndo { previous, changed })
    }
    pub(crate) fn restored(self, undo: ToolObservationUndo) -> Self {
        let Self {
            title,
            kind,
            status,
            locations,
            content,
            mcp_tool,
        } = self;
        let ToolObservationUndo { previous, changed } = undo;
        Self {
            title: if changed[0] { previous.title } else { title },
            kind: if changed[1] { previous.kind } else { kind },
            status: if changed[2] { previous.status } else { status },
            locations: if changed[3] {
                previous.locations
            } else {
                locations
            },
            content: if changed[4] {
                previous.content
            } else {
                content
            },
            mcp_tool: if changed[5] {
                previous.mcp_tool
            } else {
                mcp_tool
            },
        }
    }
}
/// The largest structured result retained, in UTF-8 bytes of its JSON text.
/// A result past it is not kept; the adapter says so in text instead.
pub const MAX_STRUCTURED_RESULT_BYTES: usize = 64 * 1024;

/// Immutable provider-reported text, file change, or structured result for
/// observation and review. Construction preserves exact text, including empty
/// strings, and compacts text allocations. This value never applies a change or
/// authorizes filesystem access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolContent {
    value: ToolContentValue,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum ToolContentValue {
    Text(Box<str>),
    Structured(Box<str>),
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
    /// A tool's structured result (MCP `structuredContent`), as the JSON text
    /// the adapter wrote it in. Beside the result's text, never instead of it.
    Structured(&'a str),
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
    /// Own a tool's structured result as `json`, the JSON text an adapter
    /// serialized it to, kept exactly. Its syntax is checked (one JSON value,
    /// RFC 8259) so nothing downstream receives text it cannot read as JSON;
    /// nothing in the domain reads inside it.
    ///
    /// # Errors
    ///
    /// [`ExecutionError::ValueTooLong`] past [`MAX_STRUCTURED_RESULT_BYTES`],
    /// checked first, and [`ExecutionError::InvalidStructuredResult`] for text
    /// that is not one JSON value.
    pub fn structured(json: impl Into<String>) -> Result<Self, ExecutionError> {
        let json = json.into();
        if json.len() > MAX_STRUCTURED_RESULT_BYTES {
            return Err(ExecutionError::ValueTooLong {
                field: "structured tool result",
                max_bytes: MAX_STRUCTURED_RESULT_BYTES,
            });
        }
        if !is_json(&json) {
            return Err(ExecutionError::InvalidStructuredResult);
        }
        Ok(Self {
            value: ToolContentValue::Structured(json.into_boxed_str()),
        })
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
            ToolContentValue::Structured(json) => ToolContentView::Structured(json),
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
            ToolContentValue::Text(text) | ToolContentValue::Structured(text) => text.len(),
            ToolContentValue::Diff { path, old, new } => path
                .0
                .capacity()
                .saturating_add(old.as_ref().map_or(0, |text| text.len()))
                .saturating_add(new.len()),
        }
    }
}

#[cfg(test)]
mod reversible_tests {
    use super::*;

    fn content_pointer(observation: &ToolObservation) -> *const ToolContent {
        observation.content().as_ref().unwrap().as_ptr()
    }

    #[test]
    fn sparse_replacement_and_rollback_move_existing_large_payloads() {
        let id = ToolCallId::new("tool").unwrap();
        let original = ToolObservation::default().with_update(ToolCallUpdate::new(
            id.clone(),
            Some("t".repeat(1024 * 1024)),
            None,
            None,
            None,
            Some(vec![ToolContent::text("c".repeat(1024 * 1024))]),
        ));
        let title = original.title().as_ref().unwrap().as_ptr();
        let content = content_pointer(&original);
        let (replacement, undo) = original.with_reversible_update(ToolCallUpdate::new(
            id.clone(),
            Some("new title".into()),
            None,
            Some(ToolStatus::Running),
            None,
            None,
        ));
        assert_eq!(content_pointer(&replacement), content);
        assert_eq!(undo.previous.title().as_ref().unwrap().as_ptr(), title);
        assert!(undo.previous.content().is_none());
        let restored = replacement.restored(undo);
        assert_eq!(restored.title().as_ref().unwrap().as_ptr(), title);
        assert_eq!(content_pointer(&restored), content);
        assert_eq!(restored.status(), &None);
        let (cleared, undo) = restored.with_reversible_update(ToolCallUpdate::new(
            id,
            None,
            None,
            None,
            None,
            Some(Vec::new()),
        ));
        assert!(cleared.content().as_ref().unwrap().is_empty());
        assert_eq!(content_pointer(&undo.previous), content);
        let restored = cleared.restored(undo);
        assert_eq!(restored.title().as_ref().unwrap().as_ptr(), title);
        assert_eq!(content_pointer(&restored), content);
    }

    #[test]
    fn an_observation_rebuilt_from_its_update_is_the_same_observation() {
        let id = ToolCallId::new("tool").unwrap();
        let full = ToolObservation::default().with_update(
            ToolCallUpdate::new(
                id.clone(),
                Some("title".into()),
                Some(ToolKind::Fetch),
                Some(ToolStatus::Failed),
                Some(vec![FileLocation::new(
                    FilePath::new("a").unwrap(),
                    Some(2),
                )]),
                Some(vec![
                    ToolContent::text("text"),
                    ToolContent::structured("{}").unwrap(),
                ]),
            )
            .with_mcp_tool(McpTool::new("server", "tool").unwrap()),
        );
        let update = full.as_update(id.clone());
        assert_eq!(update.id(), &id);
        assert_eq!(ToolObservation::default().with_update(update), full);
        let empty = ToolObservation::default();
        assert_eq!(empty.clone().with_update(empty.as_update(id)), empty);
    }

    #[test]
    fn rollback_restores_the_mcp_identity_an_update_replaced_or_left() {
        let id = ToolCallId::new("tool").unwrap();
        let bare = || ToolCallUpdate::new(id.clone(), None, None, None, None, None);
        let first = McpTool::new("first", "tool").unwrap();
        let second = McpTool::new("second", "tool").unwrap();
        let original = ToolObservation::default().with_update(bare().with_mcp_tool(first.clone()));
        let (replaced, undo) =
            original.with_reversible_update(bare().with_mcp_tool(second.clone()));
        assert_eq!(replaced.mcp_tool(), Some(&second));
        assert_eq!(replaced.restored(undo).mcp_tool(), Some(&first));
        let original = ToolObservation::default().with_update(bare().with_mcp_tool(first.clone()));
        let (kept, undo) = original.with_reversible_update(bare());
        assert_eq!(kept.mcp_tool(), Some(&first));
        assert_eq!(kept.restored(undo).mcp_tool(), Some(&first));
        let (named, undo) =
            ToolObservation::default().with_reversible_update(bare().with_mcp_tool(first));
        assert!(named.mcp_tool().is_some());
        assert_eq!(named.restored(undo).mcp_tool(), None);
    }
}
