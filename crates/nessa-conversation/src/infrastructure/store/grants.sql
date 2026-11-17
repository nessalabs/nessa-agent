-- Which paired party may do what in which resource (issues 704, 707, 889).
-- A row is the access grant; revoking removes it. A paired subject is its
-- receiver; an unpaired subject is its principal. This slice writes kind `conversation`
-- and the conversation id. A directory grant uses kind `directory` and a
-- directory id under the same key. `receiver_id` is the receiver a paired
-- device or gateway is bound to, because admission is still that receiver;
-- an integration, and a later agent, store the principal and no receiver.
-- The credential is the one the owner chose. A conversation's `capabilities`
-- is `read`, `read+comment`, or `read+comment+drive`. A directory's is a
-- non-empty subset of `read`, `write`, and `exec` in that order. A drive
-- grant carries its tool policy. A directory grant carries its command
-- matchers, or none. `policy_revision` is the grant epoch, the journal sequence
-- of the change that last set the capabilities or the constraints.
CREATE TABLE share_grants (
    principal_id TEXT NOT NULL,
    resource_kind TEXT NOT NULL CHECK (resource_kind IN ('conversation', 'directory')),
    resource_id TEXT NOT NULL,
    receiver_id TEXT,
    credential_id TEXT NOT NULL,
    capabilities TEXT NOT NULL CHECK (capabilities IN (
        'read', 'read+comment', 'read+comment+drive',
        'write', 'exec', 'read+write', 'read+exec', 'write+exec', 'read+write+exec')),
    policy TEXT,
    policy_revision INTEGER NOT NULL REFERENCES share_grant_changes (sequence),
    granted_at_ms INTEGER NOT NULL,
    subject_kind TEXT GENERATED ALWAYS AS
        (CASE WHEN receiver_id IS NULL THEN 'principal' ELSE 'receiver' END) STORED,
    subject_id TEXT GENERATED ALWAYS AS (coalesce(receiver_id, principal_id)) STORED,
    CHECK (
        (resource_kind = 'conversation'
            AND capabilities IN ('read', 'read+comment', 'read+comment+drive')
            AND (capabilities = 'read+comment+drive') = (policy IS NOT NULL))
        OR
        (resource_kind = 'directory'
            AND capabilities IN (
                'read', 'write', 'exec', 'read+write', 'read+exec',
                'write+exec', 'read+write+exec'))
    ),
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
    before TEXT NOT NULL CHECK (before IN (
        'none', 'read', 'read+comment', 'read+comment+drive',
        'write', 'exec', 'read+write', 'read+exec', 'write+exec', 'read+write+exec')),
    after TEXT NOT NULL CHECK (after IN (
        'none', 'read', 'read+comment', 'read+comment+drive',
        'write', 'exec', 'read+write', 'read+exec', 'write+exec', 'read+write+exec')),
    policy_before TEXT,
    policy_after TEXT,
    initiator TEXT NOT NULL,
    surface TEXT NOT NULL,
    request TEXT NOT NULL,
    changed_at_ms INTEGER NOT NULL,
    revision INTEGER NOT NULL,
    CHECK (
        (resource_kind = 'conversation'
            AND before IN ('none', 'read', 'read+comment', 'read+comment+drive')
            AND (before = 'read+comment+drive') = (policy_before IS NOT NULL))
        OR
        (resource_kind = 'directory'
            AND before IN (
                'none', 'read', 'write', 'exec', 'read+write', 'read+exec',
                'write+exec', 'read+write+exec'))
    ),
    CHECK (
        (resource_kind = 'conversation'
            AND after IN ('none', 'read', 'read+comment', 'read+comment+drive')
            AND (after = 'read+comment+drive') = (policy_after IS NOT NULL))
        OR
        (resource_kind = 'directory'
            AND after IN (
                'none', 'read', 'write', 'exec', 'read+write', 'read+exec',
                'write+exec', 'read+write+exec'))
    )
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

-- One directory resource. The id is minted here; the path is not the id.
-- The binding is the 32 bytes in `.nessa-binding` on that canonical path.
CREATE TABLE directory_resources (
    resource_id TEXT PRIMARY KEY,
    environment TEXT NOT NULL,
    canonical_path TEXT NOT NULL,
    binding BLOB NOT NULL CHECK (length(binding) = 32),
    UNIQUE (environment, canonical_path),
    UNIQUE (binding)
) STRICT;

-- A marker this attempt created whose resource row did not commit.
-- The row does not say the file is gone. It stays until the unlink and the
-- cleanup audit have both been recorded.
CREATE TABLE directory_cleanup (
    environment TEXT NOT NULL,
    canonical_path TEXT NOT NULL,
    binding BLOB NOT NULL CHECK (length(binding) = 32),
    unlinked INTEGER NOT NULL CHECK (unlinked IN (0, 1)),
    audit_done INTEGER NOT NULL CHECK (audit_done IN (0, 1)),
    initiator TEXT NOT NULL,
    PRIMARY KEY (environment, canonical_path)
) STRICT;

-- The first open that found a missing or different marker. Later opens keep
-- this cause. `audit_done` stays 0 until the audit sink accepts it.
CREATE TABLE directory_open_audits (
    resource_id TEXT PRIMARY KEY,
    cause TEXT NOT NULL,
    initiator TEXT NOT NULL,
    audit_done INTEGER NOT NULL CHECK (audit_done IN (0, 1))
) STRICT;

PRAGMA user_version = 2;
