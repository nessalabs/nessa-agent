# Private local database

Opens one private SQLite file for one context. The context owns its tables and
its schema; this crate owns only how the file is opened and at which version.
Decided in [ADR 196](../../docs/adr/todo/196-conversation-metadata-database.md)
and [ADR 202](../../docs/adr/todo/202-versioned-local-datasets.md).

- The directory must already be private and owned by this OS user, and the file
  is created private (through `nessa-local-storage`, never through a link)
  before SQLite opens it. SQLite is not allowed to create it. SQLite then opens
  it by name, so a process running as this same user could swap the name in
  between; that is not guarded against, since such a process can read the file
  anyway. What this keeps out is every other user.
- A file at another version is refused before anything persistent in it
  changes, its journal mode included.
- Every connection enforces foreign keys, keeps a rollback journal synced in
  full, and overwrites deleted rows (`secure_delete`). It uses a rollback journal
  rather than a write-ahead log so that erased data leaves no readable page
  image behind.
- A `Schema` states its version once, as the one `PRAGMA user_version = N;` its
  definition runs. An empty file is given the schema in one transaction; a file
  at that version is opened; any other version, or tables without a version, are
  refused as `OpenError::Version` and left untouched. A file that is not a
  database is `OpenError::Unreadable`, and a file at the schema's version that
  SQLite's `quick_check` finds damaged is `OpenError::Damaged`. That check reads
  every page once, at open. Both describe the file,
  not the moment, so a caller can refuse them for good
  ([ADR 202](../../docs/adr/todo/202-versioned-local-datasets.md)). There are no
  in-place migrations during alpha.

## Module map

| Path | Responsibility |
| --- | --- |
| `src/lib.rs` | `Schema`, `OpenError`, and `open`: private file creation, connection pragmas, and the version check. Re-exports `rusqlite` so every context uses the one version. |
| `tests/open.rs` | Schema application and reopening, version refusal, the stated-version rule, pragmas and erasure, and private-file and private-directory refusal. |
