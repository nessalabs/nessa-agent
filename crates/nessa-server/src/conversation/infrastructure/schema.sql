-- Conversation metadata. The one definition, version included (the last
-- line): the server includes this file (`store.rs`), and
-- `scripts/move-conversation-metadata.mjs` runs it to create the same file.
-- See docs/adr/todo/196-conversation-metadata-database.md.
--
-- Columns hold stored text and numbers, and a flag is 0 or 1, the file's
-- spelling of a boolean. What a valid value is — an identity, a title's
-- length, a time a list can carry — is the domain's, and is checked when a
-- row is read back, never here.

-- Who owns a conversation, and which agent it runs on. Written once.
CREATE TABLE conversations (
    id TEXT PRIMARY KEY NOT NULL,
    organization TEXT NOT NULL,
    owner TEXT NOT NULL,
    creator_surface TEXT NOT NULL,
    creation_action TEXT NOT NULL,
    creation_requested_at_ms INTEGER NOT NULL,
    agent TEXT NOT NULL
) STRICT;
CREATE INDEX conversations_by_owner ON conversations (organization, owner);

-- A deleted conversation's tombstone. Written at the fence and carried
-- further until `erased`; never removed.
CREATE TABLE deletions (
    conversation_id TEXT PRIMARY KEY NOT NULL REFERENCES conversations (id),
    organization TEXT NOT NULL,
    initiator TEXT NOT NULL,
    surface TEXT NOT NULL,
    request TEXT NOT NULL,
    requested_at_ms INTEGER NOT NULL,
    -- unread | absent | recorded | unknown; `provider_session_id` only with
    -- recorded.
    provider_session TEXT NOT NULL,
    provider_session_id TEXT,
    -- NULL until the agent's own record of the session is settled.
    provider_erasure TEXT,
    erased INTEGER NOT NULL CHECK (erased IN (0, 1))
) STRICT;
CREATE INDEX unfinished_deletions ON deletions (conversation_id) WHERE erased = 0;

-- What a list shows about a conversation. Replaced on every change; removed
-- when the conversation's deletion erases it.
CREATE TABLE summaries (
    conversation_id TEXT PRIMARY KEY NOT NULL REFERENCES conversations (id),
    title TEXT,
    preview TEXT,
    updated_at_ms INTEGER NOT NULL,
    archived INTEGER NOT NULL CHECK (archived IN (0, 1))
) STRICT;

PRAGMA user_version = 1;
