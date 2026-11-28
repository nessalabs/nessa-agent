-- A person's channels (`ChannelStore`), and which conversation is in which.
-- A conversation with no row in `channel_conversations` is in the default
-- channel, which is never stored; its coordinator is kept, by owner, in
-- `default_coordinators`. A channel's id is made from its owner and
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
    -- Whether its coordinator is on; off keeps its main conversation.
    coordinator_on INTEGER NOT NULL DEFAULT 1 CHECK (coordinator_on IN (0, 1)),
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
-- How agents talk: a person's own choices (channel '' is their Nessa
-- level), and the style each conversation was started with.
CREATE TABLE reply_styles (
    organization_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    channel TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('coordinator', 'thread')),
    style TEXT NOT NULL CHECK (style IN ('plain', 'off', 'custom')),
    text TEXT CHECK ((style = 'custom') = (text IS NOT NULL) AND length(text) > 0 AND length(CAST(text AS BLOB)) <= 4096),
    PRIMARY KEY (organization_id, owner_id, channel, role)
) STRICT, WITHOUT ROWID;
CREATE TABLE conversation_reply_styles (
    conversation TEXT PRIMARY KEY NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('coordinator', 'thread')),
    style TEXT NOT NULL CHECK (style IN ('plain', 'off', 'custom')),
    text TEXT CHECK ((style = 'custom') = (text IS NOT NULL) AND length(text) > 0 AND length(CAST(text AS BLOB)) <= 4096),
    -- The job its role was told when it was created (a coordinator's), kept
    -- as told: a conversation from before jobs has none and is told none.
    job TEXT CHECK (job IS NULL OR (length(job) > 0 AND length(CAST(job AS BLOB)) <= 8192))
) STRICT, WITHOUT ROWID;
-- The default channel's coordinator, one for each owner, made on the first
-- time the owner's channels are read.
CREATE TABLE default_coordinators (
    organization_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    coordinator TEXT NOT NULL UNIQUE,
    coordinator_on INTEGER NOT NULL DEFAULT 1 CHECK (coordinator_on IN (0, 1)),
    PRIMARY KEY (organization_id, owner_id)
) STRICT, WITHOUT ROWID;
PRAGMA user_version = 9;
