-- A channel's run location can change after it is made. Moving its main
-- agent retires the current main conversation (it stays in the channel as
-- an ordinary thread) and names a fresh one, recorded as a replacement.
CREATE TABLE channel_coordinator_replacements_next (
    retired TEXT PRIMARY KEY NOT NULL,
    channel TEXT NOT NULL REFERENCES channels (channel),
    coordinator TEXT NOT NULL UNIQUE,
    generation INTEGER NOT NULL CHECK (generation > 0),
    cause TEXT NOT NULL CHECK (cause IN ('coordinator_conversation_erased', 'coordinator_moved')),
    UNIQUE (channel, generation)
) STRICT;
INSERT INTO channel_coordinator_replacements_next (retired, channel, coordinator, generation, cause)
    SELECT retired, channel, coordinator, generation, cause FROM channel_coordinator_replacements;
DROP TABLE channel_coordinator_replacements;
ALTER TABLE channel_coordinator_replacements_next RENAME TO channel_coordinator_replacements;
PRAGMA user_version = 4;
