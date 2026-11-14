-- Version 6 to 7: a file from before #974 gains `unfinished_turns`, as
-- schema.sql creates it. Nothing else changed shape.
CREATE TABLE unfinished_turns (
    conversation_id TEXT PRIMARY KEY NOT NULL REFERENCES conversations (id),
    failures INTEGER NOT NULL DEFAULT 0 CHECK (failures >= 0),
    retry_at_ms INTEGER CHECK (retry_at_ms >= 0)
) STRICT, WITHOUT ROWID;

PRAGMA user_version = 7;
