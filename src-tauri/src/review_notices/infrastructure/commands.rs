//! The commands the desktop window calls. The JSON is the window's contract
//! with `src/host/window.ts`.
//!
//! Every call may wait on the system, so it runs on the blocking pool and the
//! window thread only waits for the join.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::composition::HostDependencies;
use crate::review_notices::application::ReviewNotices;
use crate::review_notices::domain::{NoticeAccess, NoticeId, ReviewNotice};

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AccessWire {
    Unsupported,
    NotAsked,
    Allowed,
    Denied,
}

impl From<NoticeAccess> for AccessWire {
    fn from(access: NoticeAccess) -> Self {
        match access {
            NoticeAccess::Unsupported => Self::Unsupported,
            NoticeAccess::NotAsked => Self::NotAsked,
            NoticeAccess::Allowed => Self::Allowed,
            NoticeAccess::Denied => Self::Denied,
        }
    }
}

/// One waiting review, named the way `permission.reviewWaiting` names it,
/// and what its notice says.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReviewWire {
    conversation_id: String,
    execution_id: String,
    permission_id: String,
    title: String,
    body: String,
}

impl ReviewWire {
    fn notice(&self) -> Result<ReviewNotice, String> {
        let id = NoticeId::new(
            &self.conversation_id,
            &self.execution_id,
            &self.permission_id,
        )
        .map_err(|_| "a review's ids are malformed".to_owned())?;
        Ok(ReviewNotice::new(id, &self.title, &self.body))
    }
}

async fn on_pool<T: Send + 'static>(
    notices: Arc<ReviewNotices>,
    call: impl FnOnce(&ReviewNotices) -> T + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || call(&notices))
        .await
        .map_err(|error| format!("review notice task stopped: {error}"))
}

/// Whether the system lets this app show notices.
#[tauri::command]
pub(crate) async fn review_notice_access(
    deps: State<'_, HostDependencies>,
) -> Result<AccessWire, String> {
    on_pool(Arc::clone(&deps.review_notices), |notices| {
        notices.access().into()
    })
    .await
}

/// Ask the person to allow notices. Called from Settings' own button.
#[tauri::command]
pub(crate) async fn review_notice_ask(
    deps: State<'_, HostDependencies>,
) -> Result<AccessWire, String> {
    on_pool(Arc::clone(&deps.review_notices), |notices| {
        notices.ask().into()
    })
    .await
}

/// Make the notices that are up the ones for the reviews that are waiting.
/// The window sends every set the gateway pushes, the empty one included.
/// A malformed review is left out, so it cannot keep the rest from settling.
#[tauri::command]
pub(crate) async fn review_notice_sync(
    reviews: Vec<ReviewWire>,
    deps: State<'_, HostDependencies>,
) -> Result<(), String> {
    let waiting = reviews
        .iter()
        .filter_map(|review| {
            review
                .notice()
                .inspect_err(|error| crate::host_log::warn(format!("{error}; no notice for it")))
                .ok()
        })
        .collect::<Vec<_>>();
    on_pool(Arc::clone(&deps.review_notices), move |notices| {
        notices.sync(waiting);
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_is_a_kebab_tag() {
        let tags: Vec<String> = [
            NoticeAccess::Unsupported,
            NoticeAccess::NotAsked,
            NoticeAccess::Allowed,
            NoticeAccess::Denied,
        ]
        .into_iter()
        .map(|access| serde_json::to_value(AccessWire::from(access)).unwrap())
        .map(|value| value.as_str().unwrap().to_owned())
        .collect();
        assert_eq!(tags, published_access_tags());
    }

    #[test]
    fn a_review_is_named_by_its_three_ids() {
        let wire: ReviewWire = serde_json::from_str(
            r#"{"conversationId":"c","executionId":"e","permissionId":"p","title":"t","body":"b"}"#,
        )
        .unwrap();
        let notice = wire.notice().unwrap();
        assert_eq!(notice.id, NoticeId::new("c", "e", "p").unwrap());
        assert_eq!((notice.title.as_str(), notice.body.as_str()), ("t", "b"));
        assert!(serde_json::from_str::<ReviewWire>(
            r#"{"conversationId":"c","executionId":"e","permissionId":"p","title":"t","body":"b","tool":"x"}"#,
        )
        .is_err());
        let empty: ReviewWire = serde_json::from_str(
            r#"{"conversationId":"","executionId":"e","permissionId":"p","title":"t","body":"b"}"#,
        )
        .unwrap();
        assert!(empty.notice().is_err());
    }

    /// `NOTICE_ACCESS` in `src/host/window.ts` is the list the window accepts.
    /// A tag added or renamed on only one side fails here.
    fn published_access_tags() -> Vec<String> {
        let source = include_str!("../../../../src/host/window.ts");
        let marker = "export const NOTICE_ACCESS = [";
        let start = source.find(marker).expect("NOTICE_ACCESS") + marker.len();
        let end = start + source[start..].find(']').expect("NOTICE_ACCESS end");
        source[start..end]
            .split(',')
            .map(|tag| tag.trim().trim_matches('"').to_owned())
            .filter(|tag| !tag.is_empty())
            .collect()
    }
}
