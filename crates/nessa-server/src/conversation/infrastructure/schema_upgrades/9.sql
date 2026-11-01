-- Version 9 to 10: `closed_threads`, as schema.sql creates it.
CREATE TABLE closed_threads (
    conversation_id TEXT PRIMARY KEY NOT NULL REFERENCES conversations (id)
) STRICT, WITHOUT ROWID;

PRAGMA user_version = 10;
