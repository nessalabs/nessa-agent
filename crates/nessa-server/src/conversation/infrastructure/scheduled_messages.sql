-- One-shot continuations for a provider usage limit, and the gateway's
-- preference for sending them (`scheduled-chat-messages.sqlite3`).
-- A row is written when a schedule is accepted and kept after it ends, so a
-- second delivery of the same causal turn cannot mint another execution.
-- retry_at_ms is 0 when a submitting row may be attempted now. A later
-- instant is the next attempt. retry_failures counts attempts already
-- deferred. reconcile_retry is a settlement whose plan did not commit.
-- preference_command is one request's receipt. It commits with that
-- request's decision audit. An enable is pending until its scan settles
-- or its rollback commits. A retry of pending resumes that work. stored
-- is written only when that work has finished, or when a disable or a
-- deny finished in the decision transaction. installed_request names the
-- enable a revert marks failed. A scan that fails marks that request
-- failed even when it installed nothing. A settle that loses to a newer
-- preference revision marks that request failed and leaves the newer
-- revision. A revert that does not commit leaves the receipt pending.
-- history_fence is the compare-and-set token a plan observed: incarnation
-- and position, the generation number of the fence at that read, and the
-- agent and model. A different incarnation, a later position, or a different
-- agent or model replaces the fence only when it observed the generation now
-- stored, and that number then advances. The same incarnation and position
-- are current only while the generation and the agent and model still match.
-- An older generation writes nothing, including when the model has changed
-- back. Cancel, move, defer, accept, and a hold that dismisses an open row
-- compare that token in the same transaction as the row change. A miss
-- writes nothing. Accept and settle of a submitting row also match the owner
-- of that write. A cancelled
-- row for the same causal turn can be scheduled again unless an agent or
-- model change cancelled it, as retained by its cancellation audit. delivered, refused,
-- and unconfirmed cannot. A submitting row another process still leases is
-- not due until that lease's deadline.
-- user_version stays 1. An earlier copy of this file is recreated.
CREATE TABLE preference (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    revision INTEGER NOT NULL DEFAULT 0,
    settled INTEGER NOT NULL DEFAULT 1 CHECK (settled IN (0, 1)),
    installed_request TEXT NOT NULL DEFAULT ''
) STRICT;

CREATE TABLE preference_command (
    request_id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL,
    principal_id TEXT NOT NULL,
    surface_id TEXT NOT NULL,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    outcome TEXT NOT NULL CHECK (outcome IN ('stored', 'refused', 'failed', 'pending')),
    result_enabled INTEGER NOT NULL CHECK (result_enabled IN (0, 1)),
    policies TEXT NOT NULL DEFAULT ''
) STRICT;

CREATE TABLE message (
    conversation TEXT NOT NULL,
    causal_execution TEXT NOT NULL,
    execution_id TEXT NOT NULL UNIQUE,
    due_at_ms INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (
        status IN (
            'scheduled',
            'delivering',
            'submitting',
            'delivered',
            'cancelled',
            'refused',
            'unconfirmed'
        )
    ),
    agent TEXT NOT NULL,
    model TEXT NOT NULL,
    prompt TEXT NOT NULL,
    organization_id TEXT NOT NULL,
    principal_id TEXT NOT NULL,
    surface_id TEXT NOT NULL,
    action_id TEXT NOT NULL,
    initiation TEXT NOT NULL CHECK (initiation IN ('owner', 'grant')),
    grant_id TEXT,
    policy_revision INTEGER,
    tools_checked INTEGER NOT NULL CHECK (tools_checked IN (0, 1)),
    fence_owner TEXT NOT NULL DEFAULT '',
    fence_until_ms INTEGER NOT NULL DEFAULT 0,
    retry_at_ms INTEGER NOT NULL DEFAULT 0,
    retry_failures INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (conversation, causal_execution)
) STRICT;

CREATE UNIQUE INDEX message_one_open ON message (conversation)
    WHERE status IN ('scheduled', 'delivering', 'submitting');

CREATE TABLE reconcile_retry (
    conversation TEXT PRIMARY KEY,
    failures INTEGER NOT NULL,
    retry_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE history_fence (
    conversation TEXT PRIMARY KEY,
    incarnation TEXT NOT NULL,
    position INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    agent TEXT NOT NULL,
    model TEXT NOT NULL
) STRICT;

-- One applied model request invalidates only the limit it first observed.
-- Empty causal_execution records that there was no limit at that boundary.
CREATE TABLE usage_limit_invalidation (
    conversation TEXT NOT NULL,
    request_id TEXT NOT NULL,
    incarnation TEXT NOT NULL,
    causal_execution TEXT NOT NULL,
    PRIMARY KEY (conversation, request_id)
) STRICT;

CREATE INDEX usage_limit_invalidation_causal ON usage_limit_invalidation
    (conversation, incarnation, causal_execution);

CREATE TABLE audit (
    id INTEGER PRIMARY KEY,
    at_ms INTEGER NOT NULL,
    conversation TEXT NOT NULL,
    causal_execution TEXT NOT NULL,
    execution_id TEXT NOT NULL,
    before_status TEXT,
    after_status TEXT NOT NULL,
    cause TEXT NOT NULL,
    initiator_kind TEXT NOT NULL CHECK (initiator_kind IN ('system', 'person')),
    organization_id TEXT,
    principal_id TEXT,
    surface_id TEXT,
    action_id TEXT,
    policies TEXT NOT NULL DEFAULT ''
) STRICT;

PRAGMA user_version = 1;
