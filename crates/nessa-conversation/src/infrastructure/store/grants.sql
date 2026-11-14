-- Which paired party may do what in which resource (issues 704, 707, 889).
-- A row is the access grant; revoking removes it. A paired subject is its
-- receiver; an unpaired subject is its principal. This slice writes kind `conversation`
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
    subject_kind TEXT GENERATED ALWAYS AS
        (CASE WHEN receiver_id IS NULL THEN 'principal' ELSE 'receiver' END) STORED,
    subject_id TEXT GENERATED ALWAYS AS (coalesce(receiver_id, principal_id)) STORED,
    CHECK ((capabilities = 'read+comment+drive') = (policy IS NOT NULL)),
    UNIQUE (subject_kind, subject_id, resource_kind, resource_id),
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


-- Attribution has no capability and is bound to this enrollment's credential.
CREATE TABLE person_links (
    organization_id TEXT NOT NULL,
    gateway_principal TEXT NOT NULL,
    person_principal TEXT NOT NULL,
    credential_id TEXT NOT NULL UNIQUE,
    invitation TEXT NOT NULL,
    PRIMARY KEY (organization_id, gateway_principal)
) STRICT;
CREATE TABLE person_link_changes (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    organization_id TEXT NOT NULL,
    gateway_principal TEXT NOT NULL,
    credential_id TEXT NOT NULL,
    invitation TEXT NOT NULL,
    person_before TEXT,
    person_after TEXT,
    cause TEXT NOT NULL,
    initiator_kind TEXT NOT NULL,
    initiator_id TEXT,
    observed_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX person_link_changes_credential ON person_link_changes (credential_id, sequence);
PRAGMA user_version = 1;
