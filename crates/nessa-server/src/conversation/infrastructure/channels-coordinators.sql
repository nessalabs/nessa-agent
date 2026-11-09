-- A replacement is recorded only after its predecessor's conversation was erased.
-- The predecessor identifies the committed deletion and its actual initiator.
CREATE TABLE channel_coordinator_replacements (
    retired TEXT PRIMARY KEY NOT NULL,
    channel TEXT NOT NULL REFERENCES channels (channel),
    coordinator TEXT NOT NULL UNIQUE,
    generation INTEGER NOT NULL CHECK (generation > 0),
    cause TEXT NOT NULL CHECK (cause = 'coordinator_conversation_erased'),
    UNIQUE (channel, generation)
) STRICT;
PRAGMA user_version = 3;
