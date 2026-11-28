-- The default channel's coordinator, one for each owner (#1509). The default
-- channel is never stored, so its main conversation is kept here, by its
-- owner. A store that held channels before this has no row: one is made, on,
-- the first time the owner's channels are read, so every existing owner's
-- Conversations gains a coordinator and their chats there stay threads.
CREATE TABLE default_coordinators (
    organization_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    coordinator TEXT NOT NULL UNIQUE,
    coordinator_on INTEGER NOT NULL DEFAULT 1 CHECK (coordinator_on IN (0, 1)),
    PRIMARY KEY (organization_id, owner_id)
) STRICT, WITHOUT ROWID;
-- A default coordinator's replacement, recorded in the same write that
-- erased its predecessor: which conversation retired, which took its place,
-- for whom, and why.
CREATE TABLE default_coordinator_replacements (
    retired TEXT PRIMARY KEY NOT NULL,
    organization_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    coordinator TEXT NOT NULL UNIQUE,
    cause TEXT NOT NULL CHECK (cause = 'coordinator_conversation_erased')
) STRICT;
PRAGMA user_version = 9;
