-- Version 10 to 11: `owner_asks`, as schema.sql creates it.
-- The question an agent asked its conversation's owner (`ask_owner`), one
-- per conversation: a later ask replaces it, and the next message a person
-- writes in the conversation, which is the answer, removes it
-- (docs/design/agent-thread-tools.md). `ask` is the whole ask as JSON.
CREATE TABLE owner_asks (
    conversation_id TEXT PRIMARY KEY NOT NULL REFERENCES conversations (id),
    ask TEXT NOT NULL CHECK (length(CAST(ask AS BLOB)) BETWEEN 2 AND 8192)
) STRICT, WITHOUT ROWID;

PRAGMA user_version = 11;
