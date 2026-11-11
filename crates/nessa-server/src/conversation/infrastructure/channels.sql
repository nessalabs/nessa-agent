-- A person's channels (`ChannelStore`), and which conversation is in which.
-- A conversation with no row in `channel_conversations` is in the default
-- channel, which is never stored. A channel's id is made from its owner and
-- request id, so a retried creation names the same row.
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
-- A replacement is recorded after its predecessor's conversation was erased,
-- or when the person moved the channel's main agent to another run location;
-- a moved predecessor stays in the channel as an ordinary thread.
CREATE TABLE channel_coordinator_replacements (
    retired TEXT PRIMARY KEY NOT NULL,
    channel TEXT NOT NULL REFERENCES channels (channel),
    coordinator TEXT NOT NULL UNIQUE,
    generation INTEGER NOT NULL CHECK (generation > 0),
    cause TEXT NOT NULL CHECK (cause IN ('coordinator_conversation_erased', 'coordinator_moved')),
    UNIQUE (channel, generation)
) STRICT;
PRAGMA user_version = 6;
