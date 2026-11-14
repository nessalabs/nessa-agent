-- A channel may run its agents on one named computer, not only on a kind
-- of place. SQLite cannot change a CHECK in place, so the tables are rebuilt
-- with every row kept, as the pool upgrade did: the two that refer to
-- `channels` are copied aside and dropped first.
CREATE TABLE kept_channels AS SELECT * FROM channels;
CREATE TABLE kept_channel_conversations AS SELECT * FROM channel_conversations;
CREATE TABLE kept_coordinator_replacements AS SELECT * FROM channel_coordinator_replacements;
DROP TABLE channel_coordinator_replacements;
DROP TABLE channel_conversations;
DROP TABLE channels;
CREATE TABLE channels (
    channel TEXT PRIMARY KEY NOT NULL,
    organization_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    name TEXT NOT NULL,
    folder TEXT NOT NULL,
    folder_made INTEGER NOT NULL CHECK (folder_made IN (0, 1)),
    agent TEXT NOT NULL,
    model TEXT NOT NULL,
    mode TEXT NOT NULL,
    notes TEXT NOT NULL,
    -- A JSON array of absolute paths, in the order the person gave them.
    resources TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    -- Where its main agent and its threads run: `local`, `boat`, `pool`, or
    -- one computer by its place name (ChannelExecution reads them).
    coordinator_placement TEXT NOT NULL DEFAULT 'local' CHECK (length(coordinator_placement) BETWEEN 1 AND 253),
    thread_placement TEXT NOT NULL DEFAULT 'local' CHECK (length(thread_placement) BETWEEN 1 AND 253),
    UNIQUE (organization_id, owner_id, request_id)
) STRICT;
CREATE TABLE channel_conversations (
    conversation TEXT PRIMARY KEY NOT NULL,
    channel TEXT NOT NULL REFERENCES channels (channel)
) STRICT;
CREATE TABLE channel_coordinator_replacements (
    retired TEXT PRIMARY KEY NOT NULL,
    channel TEXT NOT NULL REFERENCES channels (channel),
    coordinator TEXT NOT NULL UNIQUE,
    generation INTEGER NOT NULL CHECK (generation > 0),
    cause TEXT NOT NULL CHECK (cause IN ('coordinator_conversation_erased', 'coordinator_moved')),
    UNIQUE (channel, generation)
) STRICT;
INSERT INTO channels (channel, organization_id, owner_id, request_id, name, folder, folder_made,
                      agent, model, mode, notes, resources, created_at, coordinator_placement,
                      thread_placement)
SELECT channel, organization_id, owner_id, request_id, name, folder, folder_made, agent, model,
       mode, notes, resources, created_at, coordinator_placement, thread_placement
FROM kept_channels;
INSERT INTO channel_conversations (conversation, channel)
SELECT conversation, channel FROM kept_channel_conversations;
INSERT INTO channel_coordinator_replacements (retired, channel, coordinator, generation, cause)
SELECT retired, channel, coordinator, generation, cause FROM kept_coordinator_replacements;
DROP TABLE kept_coordinator_replacements;
DROP TABLE kept_channel_conversations;
DROP TABLE kept_channels;
PRAGMA user_version = 6;
