-- Conversation metadata. The one definition, version included (the last
-- line), which the server includes (`store.rs`).
-- See docs/adr/done/196-conversation-metadata-database.md.
--
-- Any change to what this file creates raises the version and adds the step
-- that brings the version before it forward, in `schema_upgrades/`, with
-- the same statements (docs/adr/todo/202-versioned-local-datasets.md,
-- "Migrations"). `an_upgraded_file_is_the_shape_a_new_one_is` holds the two
-- to the same shape.
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

-- One native model decision, including the approval choice the harness confirmed.
CREATE TABLE model_requests (
    conversation_id TEXT NOT NULL REFERENCES conversations (id),
    request_id TEXT NOT NULL,
    initiator TEXT NOT NULL,
    surface TEXT NOT NULL,
    prior_provider TEXT NOT NULL,
    prior_context TEXT NOT NULL,
    requested_provider TEXT NOT NULL,
    requested_context TEXT NOT NULL,
    prior_model TEXT NOT NULL,
    prior_mode TEXT NOT NULL CHECK (prior_mode IN ('ask','auto','full')),
    requested_model TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending','applied','not_applied')),
    application TEXT CHECK (application IN ('applied','refused','uncertain')),
    applied_mode TEXT CHECK (applied_mode IN ('ask','auto','full')),
    effect TEXT CHECK (effect IN ('subsequent_responses','next_turn')),
    requested_at_ms INTEGER NOT NULL,
    PRIMARY KEY (conversation_id,request_id),
    CHECK ((application = 'applied' AND applied_mode IS NOT NULL AND effect IS NOT NULL)
        OR (application IS NULL AND applied_mode IS NULL AND effect IS NULL)
        OR (application IN ('refused','uncertain') AND applied_mode IS NULL AND effect IS NULL)),
    CHECK (state <> 'applied' OR application = 'applied'),
    CHECK (state <> 'not_applied' OR application = 'refused'),
    CHECK (state = 'pending' OR application IS NOT NULL)
) STRICT;
CREATE UNIQUE INDEX one_pending_model_request ON model_requests(conversation_id) WHERE state='pending';

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

-- Which paired party may do what in which resource (issues 704, 707, 889).
-- A row is the access grant; revoking removes it. Nothing is granted by
-- default or by list. The row is keyed by the grantee's principal, the
-- resource kind, and the resource id. This slice writes kind `conversation`
-- and the conversation id. A directory grant uses kind `directory` and a
-- directory id under the same key. `receiver_id` is the receiver a paired
-- device or gateway is bound to, because admission is still that receiver;
-- an integration, and a later agent, store the principal and no receiver.
-- The credential is the one the owner chose. `capabilities` is the set,
-- spelled `read`, `read+comment`, or `read+comment+drive`. A drive grant
-- alone carries constraints: the JSON of its `ToolPolicy`, tool policy and
-- command hosts. `policy_revision` is the grant epoch, the journal sequence
-- of the change that last set the capabilities or the constraints.
CREATE TABLE share_grants (
    principal_id TEXT NOT NULL,
    resource_kind TEXT NOT NULL CHECK (resource_kind IN ('conversation', 'directory')),
    resource_id TEXT NOT NULL,
    receiver_id TEXT,
    credential_id TEXT NOT NULL,
    capabilities TEXT NOT NULL CHECK (capabilities IN ('read', 'read+comment', 'read+comment+drive')),
    policy TEXT,
    policy_revision INTEGER NOT NULL REFERENCES share_grant_changes (sequence),
    granted_at_ms INTEGER NOT NULL,
    CHECK ((capabilities = 'read+comment+drive') = (policy IS NOT NULL)),
    PRIMARY KEY (principal_id, resource_kind, resource_id),
    UNIQUE (credential_id, resource_kind, resource_id)
) STRICT;

-- One paired receiver holds one grant on a resource. A row with no
-- receiver is not in this index.
CREATE UNIQUE INDEX share_grants_receiver
    ON share_grants (receiver_id, resource_kind, resource_id)
    WHERE receiver_id IS NOT NULL;

-- One resource's grants, for its shares list, count and revoke.
CREATE INDEX share_grants_resource ON share_grants (resource_kind, resource_id);

-- Every grant, change of capabilities or constraints, and revoke as the
-- owner made it: the capabilities and constraints before and after, who,
-- from which surface, under which request, and the catalogue revision it
-- took. A request that changed nothing is kept too, with before equal to
-- after, so a retry of it replays that answer.
-- Never changed or removed.
CREATE TABLE share_grant_changes (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    principal_id TEXT NOT NULL,
    resource_kind TEXT NOT NULL CHECK (resource_kind IN ('conversation', 'directory')),
    resource_id TEXT NOT NULL,
    receiver_id TEXT,
    credential_id TEXT NOT NULL,
    before TEXT NOT NULL CHECK (before IN ('none', 'read', 'read+comment', 'read+comment+drive')),
    after TEXT NOT NULL CHECK (after IN ('none', 'read', 'read+comment', 'read+comment+drive')),
    policy_before TEXT,
    policy_after TEXT,
    initiator TEXT NOT NULL,
    surface TEXT NOT NULL,
    request TEXT NOT NULL,
    changed_at_ms INTEGER NOT NULL,
    revision INTEGER NOT NULL,
    CHECK ((before = 'read+comment+drive') = (policy_before IS NOT NULL)),
    CHECK ((after = 'read+comment+drive') = (policy_after IS NOT NULL))
) STRICT;

-- One journal row per request: a retried request is answered from it.
CREATE UNIQUE INDEX share_grant_changes_request
    ON share_grant_changes (resource_kind, resource_id, initiator, surface, request);
-- The journal by receiver, which a paired reader's catalogue head reads
-- through (`granted_head` in store.rs).
CREATE INDEX share_grant_changes_by_receiver
    ON share_grant_changes (receiver_id, revision);

-- Each conversation that may have a turn a restart would leave unfinished:
-- written before its agent is given a turn, removed once its agent has
-- nothing left to do. The startup sweep opens these and resumes what their
-- history says is owed; the history, not this table, decides what that is.
-- After a failure, `failures` counts the retries in a row and
-- `retry_at_ms` (Unix milliseconds) is when the next one is due.
CREATE TABLE unfinished_turns (
    conversation_id TEXT PRIMARY KEY NOT NULL REFERENCES conversations (id),
    failures INTEGER NOT NULL DEFAULT 0 CHECK (failures >= 0),
    retry_at_ms INTEGER CHECK (retry_at_ms >= 0)
) STRICT, WITHOUT ROWID;

PRAGMA user_version = 8;
