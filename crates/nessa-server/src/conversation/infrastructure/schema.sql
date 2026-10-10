-- Conversation metadata. The one definition, version included (the last
-- line), which the server includes (`store.rs`).
-- See docs/adr/done/196-conversation-metadata-database.md.
--
-- Columns hold stored text and numbers, and a flag is 0 or 1, the file's
-- spelling of a boolean. What a valid value is — an identity, a title's
-- length, a time a list can carry — is the domain's, and is checked when a
-- row is read back, never here.

-- The incarnation changes when a new metadata file replaces an old one.
CREATE TABLE catalogue_identity (id INTEGER PRIMARY KEY CHECK (id = 1), incarnation TEXT NOT NULL) STRICT;
INSERT INTO catalogue_identity (id, incarnation) VALUES (1, lower(hex(randomblob(16))));

-- One monotonic head per owner.
CREATE TABLE catalogue_owners (
    organization TEXT NOT NULL,
    owner TEXT NOT NULL,
    head INTEGER NOT NULL CHECK (head >= 0),
    PRIMARY KEY (organization, owner)
) STRICT;

-- Who owns a conversation, and which agent it runs on. Written once.
CREATE TABLE conversations (
    id TEXT PRIMARY KEY NOT NULL,
    organization TEXT NOT NULL,
    owner TEXT NOT NULL,
    creator_surface TEXT NOT NULL,
    creation_action TEXT NOT NULL,
    creation_requested_at_ms INTEGER NOT NULL,
    agent TEXT NOT NULL,
    model TEXT NOT NULL,
    approval_mode TEXT NOT NULL CHECK (approval_mode IN ('ask', 'auto', 'full')),
    creation_revision INTEGER NOT NULL CHECK (creation_revision > 0),
    change_revision INTEGER NOT NULL CHECK (change_revision >= creation_revision),
    FOREIGN KEY (organization, owner) REFERENCES catalogue_owners (organization, owner)
) STRICT;
CREATE INDEX conversations_by_owner ON conversations (organization, owner);
CREATE INDEX conversations_catalogue ON conversations
    (organization, owner, creation_revision, id);
CREATE INDEX conversations_change_revision ON conversations
    (organization, owner, change_revision DESC);

-- One correlated approval-mode decision. The pending row fences turn
-- admission until recovery has established the authoritative committed mode.
CREATE TABLE mode_requests (
    conversation_id TEXT NOT NULL REFERENCES conversations (id),
    request_id TEXT NOT NULL,
    initiator TEXT NOT NULL,
    surface TEXT NOT NULL,
    prior_mode TEXT NOT NULL CHECK (prior_mode IN ('ask', 'auto', 'full')),
    requested_mode TEXT NOT NULL CHECK (requested_mode IN ('ask', 'auto', 'full')),
    state TEXT NOT NULL CHECK (state IN ('pending', 'applied', 'not_applied')),
    application TEXT CHECK (application IN ('deferred', 'applied', 'refused', 'uncertain')),
    requested_at_ms INTEGER NOT NULL,
    PRIMARY KEY (conversation_id, request_id)
) STRICT;
CREATE UNIQUE INDEX one_pending_mode_request ON mode_requests (conversation_id) WHERE state = 'pending';

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

-- Which paired device may read which conversation (issue 704). A row is a
-- grant; revoking removes it. Nothing is granted by default or by list. The
-- device is named by its receiver binding and by the credential the owner
-- chose it by; both are minted when it paired and never reused.
CREATE TABLE read_grants (
    conversation_id TEXT NOT NULL REFERENCES conversations (id),
    receiver_id TEXT NOT NULL,
    credential_id TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role = 'read'),
    granted_at_ms INTEGER NOT NULL,
    PRIMARY KEY (receiver_id, conversation_id),
    UNIQUE (credential_id, conversation_id)
) STRICT;

-- A conversation's grants, for its shares list, count and revoke.
CREATE INDEX read_grants_conversation ON read_grants (conversation_id);

-- Every grant and revoke as the owner made it: before and after, who, from
-- which surface, under which request, and the catalogue revision it took. A
-- request that changed nothing is kept too, with before equal to after, so a
-- retry of it replays that answer.
-- Never changed or removed.
CREATE TABLE read_grant_changes (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id TEXT NOT NULL REFERENCES conversations (id),
    receiver_id TEXT NOT NULL,
    credential_id TEXT NOT NULL,
    before TEXT NOT NULL CHECK (before IN ('none', 'read')),
    after TEXT NOT NULL CHECK (after IN ('none', 'read')),
    initiator TEXT NOT NULL,
    surface TEXT NOT NULL,
    request TEXT NOT NULL,
    changed_at_ms INTEGER NOT NULL,
    revision INTEGER NOT NULL
) STRICT;

-- One journal row per request: a retried request is answered from it.
CREATE UNIQUE INDEX read_grant_changes_request
    ON read_grant_changes (conversation_id, initiator, surface, request);
-- Its index by receiver is RECEIVER_INDEX in store.rs, made on every open.

PRAGMA user_version = 4;
