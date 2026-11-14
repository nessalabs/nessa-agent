-- Reports waiting for a thread's turn to finish, to be queued into the
-- conversation whose agent sent it (`ThreadReportStore`). A row is written
-- before the send is submitted and deleted once the parent has the report.
CREATE TABLE waiting_reports (
    report TEXT PRIMARY KEY NOT NULL,
    child TEXT NOT NULL,
    execution TEXT NOT NULL,
    parent TEXT NOT NULL,
    organization_id TEXT NOT NULL,
    principal_id TEXT NOT NULL,
    surface_id TEXT NOT NULL,
    action_id TEXT NOT NULL
) STRICT;
CREATE INDEX waiting_reports_child ON waiting_reports (child);
CREATE INDEX waiting_reports_parent ON waiting_reports (parent);
PRAGMA user_version = 1;
