use crate::{
    application::agent_execution::{
        executions::limits::validate_observation_id, sessions::storage::StorageError,
    },
    domain::agent_execution::tools::*,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Tool {
    pub id: String,
    title: Option<String>,
    kind: Option<Kind>,
    status: Option<Status>,
    locations: Option<Vec<Location>>,
    content: Option<Vec<Content>>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Location {
    path: String,
    line: Option<u32>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Content {
    Text(String),
    Diff {
        path: String,
        old: Option<String>,
        new: String,
    },
}
#[derive(Serialize, Deserialize)]
enum Kind {
    Read,
    Edit,
    Search,
    Other,
}
#[derive(Serialize, Deserialize)]
enum Status {
    Pending,
    Running,
    Completed,
    Failed,
}
impl From<&ToolCallUpdate> for Tool {
    fn from(value: &ToolCallUpdate) -> Self {
        Self::fields(
            value.id(),
            value.title(),
            value.kind(),
            value.status(),
            value.locations(),
            value.content(),
        )
    }
}
impl Tool {
    pub(super) fn observation(id: &ToolCallId, value: &ToolObservation) -> Self {
        Self::fields(
            id,
            value.title(),
            value.kind(),
            value.status(),
            value.locations(),
            value.content(),
        )
    }
    fn fields(
        id: &ToolCallId,
        title: &Option<String>,
        kind: &Option<ToolKind>,
        status: &Option<ToolStatus>,
        locations: &Option<Vec<FileLocation>>,
        content: &Option<Vec<ToolContent>>,
    ) -> Self {
        Self {
            id: id.as_str().into(),
            title: title.clone(),
            kind: kind.map(|kind| match kind {
                ToolKind::Read => Kind::Read,
                ToolKind::Edit => Kind::Edit,
                ToolKind::Search => Kind::Search,
                ToolKind::Other => Kind::Other,
            }),
            status: status.map(|status| match status {
                ToolStatus::Pending => Status::Pending,
                ToolStatus::Running => Status::Running,
                ToolStatus::Completed => Status::Completed,
                ToolStatus::Failed => Status::Failed,
            }),
            locations: locations.as_ref().map(|values| {
                values
                    .iter()
                    .map(|value| Location {
                        path: value.path().as_str().into(),
                        line: value.line(),
                    })
                    .collect()
            }),
            content: content.as_ref().map(|values| {
                values
                    .iter()
                    .map(|value| match value.view() {
                        ToolContentView::Text(text) => Content::Text(text.into()),
                        ToolContentView::Diff { path, old, new } => Content::Diff {
                            path: path.as_str().into(),
                            old: old.map(str::to_owned),
                            new: new.into(),
                        },
                    })
                    .collect()
            }),
        }
    }
    pub(super) fn decode(self) -> Result<ToolCallUpdate, StorageError> {
        validate_observation_id(&self.id).map_err(corrupt)?;
        Ok(ToolCallUpdate::new(
            ToolCallId::new(self.id).map_err(corrupt)?,
            self.title,
            self.kind.map(|kind| match kind {
                Kind::Read => ToolKind::Read,
                Kind::Edit => ToolKind::Edit,
                Kind::Search => ToolKind::Search,
                Kind::Other => ToolKind::Other,
            }),
            self.status.map(|status| match status {
                Status::Pending => ToolStatus::Pending,
                Status::Running => ToolStatus::Running,
                Status::Completed => ToolStatus::Completed,
                Status::Failed => ToolStatus::Failed,
            }),
            self.locations
                .map(|values| {
                    values
                        .into_iter()
                        .map(|value| {
                            Ok(FileLocation::new(
                                FilePath::new(value.path).map_err(corrupt)?,
                                value.line,
                            ))
                        })
                        .collect::<Result<_, StorageError>>()
                })
                .transpose()?,
            self.content
                .map(|values| {
                    values
                        .into_iter()
                        .map(|value| {
                            Ok(match value {
                                Content::Text(text) => ToolContent::text(text),
                                Content::Diff { path, old, new } => ToolContent::diff(
                                    FilePath::new(path).map_err(corrupt)?,
                                    old,
                                    new,
                                ),
                            })
                        })
                        .collect::<Result<_, StorageError>>()
                })
                .transpose()?,
        ))
    }
    pub(super) fn decode_observation(self) -> Result<(ToolCallId, ToolObservation), StorageError> {
        let update = self.decode()?;
        let id = update.id().clone();
        Ok((id, ToolObservation::default().with_update(update)))
    }
}
pub(super) fn corrupt(error: impl std::fmt::Display) -> StorageError {
    StorageError::Corrupt(error.to_string())
}
