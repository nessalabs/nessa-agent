//! The private SQLite dataset that keeps waiting thread reports across a
//! gateway restart (`thread-reports.sqlite3`, `thread_reports.sql`).

use crate::conversation::application::{
    ConversationCaller, ConversationError, ConversationFuture, PendingReport, ThreadReport,
    ThreadReportStore,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_local_database::{
    rusqlite::{params, Connection},
    OpenError, Schema,
};
use nessa_protocol::conversation::domain::ConversationId;
use std::{
    path::Path,
    sync::{Arc, Mutex, PoisonError},
};

const DEFINITION: &str = include_str!("thread_reports.sql");

/// Waiting thread reports, one row each, keyed by the report's execution.
pub struct LocalThreadReports {
    connection: Arc<Mutex<Connection>>,
}

impl LocalThreadReports {
    /// Open the database at `path`, in a private directory owned by this OS
    /// user, creating it when absent.
    pub fn open(path: &Path) -> Result<Self, OpenError> {
        let connection = nessa_local_database::open(path, &Schema::new(DEFINITION)?)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Connection) -> Result<T, ConversationError> + Send + 'static,
    ) -> ConversationFuture<'_, T> {
        let connection = self.connection.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let mut connection = connection.lock().unwrap_or_else(PoisonError::into_inner);
                work(&mut connection)
            })
            .await
            .map_err(|_| ConversationError::Metadata)?
        })
    }
}

fn failed(error: nessa_local_database::rusqlite::Error) -> ConversationError {
    tracing::warn!(%error, "the thread report store did not answer");
    ConversationError::Metadata
}

impl ThreadReportStore for LocalThreadReports {
    fn save(&self, report: &PendingReport) -> ConversationFuture<'_, bool> {
        let report = report.clone();
        self.run(move |connection| {
            let caller = &report.report.caller;
            let written = connection
                .execute(
                    "INSERT INTO waiting_reports (report, child, execution, parent, \
                     organization_id, principal_id, surface_id, action_id) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) ON CONFLICT (report) DO NOTHING",
                    params![
                        report.report.execution,
                        report.child.to_string(),
                        report.execution,
                        report.report.parent.to_string(),
                        caller.organization_id.as_str(),
                        caller.principal_id.as_str(),
                        caller.surface_id,
                        caller.action_id,
                    ],
                )
                .map_err(failed)?;
            Ok(written == 1)
        })
    }

    fn settle(&self, report: &str) -> ConversationFuture<'_, ()> {
        let report = report.to_owned();
        self.run(move |connection| {
            connection
                .execute("DELETE FROM waiting_reports WHERE report = ?1", [report])
                .map_err(failed)?;
            Ok(())
        })
    }

    fn forget_conversation(&self, id: &ConversationId) -> ConversationFuture<'_, ()> {
        let id = id.to_string();
        self.run(move |connection| {
            connection
                .execute(
                    "DELETE FROM waiting_reports WHERE child = ?1 OR parent = ?1",
                    [id],
                )
                .map_err(failed)?;
            Ok(())
        })
    }

    fn pending(&self) -> ConversationFuture<'_, Vec<PendingReport>> {
        self.run(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT report, child, execution, parent, organization_id, principal_id, \
                     surface_id, action_id FROM waiting_reports ORDER BY child, report",
                )
                .map_err(failed)?;
            let rows = statement
                .query_map([], |row| {
                    Ok([
                        row.get::<_, String>(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ])
                })
                .map_err(failed)?;
            let mut pending = Vec::new();
            for row in rows {
                let [report, child, execution, parent, organization, principal, surface, action] =
                    row.map_err(failed)?;
                match named(&child, &parent, &organization, &principal) {
                    Some((child, parent, organization_id, principal_id)) => {
                        pending.push(PendingReport {
                            child,
                            execution,
                            report: ThreadReport {
                                parent,
                                caller: ConversationCaller {
                                    organization_id,
                                    principal_id,
                                    surface_id: surface,
                                    action_id: action,
                                },
                                execution: report,
                            },
                        });
                    }
                    // A row this build cannot name is left as it is.
                    None => {
                        tracing::warn!(report, "a saved thread report names an invalid identity")
                    }
                }
            }
            Ok(pending)
        })
    }
}

fn named(
    child: &str,
    parent: &str,
    organization: &str,
    principal: &str,
) -> Option<(ConversationId, ConversationId, OrganizationId, PrincipalId)> {
    Some((
        ConversationId::new(child).ok()?,
        ConversationId::new(parent).ok()?,
        OrganizationId::new(organization).ok()?,
        PrincipalId::new(principal).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(child: &str, parent: &str, id: &str) -> PendingReport {
        PendingReport {
            child: ConversationId::new(child).unwrap(),
            execution: format!("{id}-turn"),
            report: ThreadReport {
                parent: ConversationId::new(parent).unwrap(),
                caller: ConversationCaller {
                    organization_id: OrganizationId::new("org").unwrap(),
                    principal_id: PrincipalId::new("person").unwrap(),
                    surface_id: "agent".into(),
                    action_id: format!("{id}-action"),
                },
                execution: id.into(),
            },
        }
    }

    /// Saved reports outlive the store that saved them; a second save of one
    /// changes nothing, and settling or deleting a conversation lets go.
    #[tokio::test]
    async fn waiting_reports_survive_reopening_until_settled() {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("conversations");
        nessa_local_storage::create_directory(&private).unwrap();
        let path = private.join("thread-reports.sqlite3");
        let child = "00000000-0000-4000-8000-000000000001";
        let parent = "00000000-0000-4000-8000-000000000002";
        let other = "00000000-0000-4000-8000-000000000003";
        {
            let store = LocalThreadReports::open(&path).unwrap();
            assert!(store.save(&report(child, parent, "a")).await.unwrap());
            assert!(!store.save(&report(child, parent, "a")).await.unwrap());
            store.save(&report(child, parent, "b")).await.unwrap();
            store.save(&report(other, child, "c")).await.unwrap();
        }
        let store = LocalThreadReports::open(&path).unwrap();
        let pending = store.pending().await.unwrap();
        let names: Vec<_> = pending
            .iter()
            .map(|p| p.report.execution.as_str())
            .collect();
        assert_eq!(names, ["a", "b", "c"]);
        assert_eq!(pending[0].execution, "a-turn");
        assert_eq!(pending[0].report.caller.action_id, "a-action");
        assert_eq!(pending[0].report.parent.to_string(), parent);
        store.settle("a").await.unwrap();
        store.settle("missing").await.unwrap();
        let left: Vec<_> = store
            .pending()
            .await
            .unwrap()
            .into_iter()
            .map(|p| p.report.execution)
            .collect();
        assert_eq!(left, ["b", "c"]);
        store
            .forget_conversation(&ConversationId::new(child).unwrap())
            .await
            .unwrap();
        assert!(store.pending().await.unwrap().is_empty());
    }
}
