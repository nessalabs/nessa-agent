-- One-shot continuations for a provider usage limit, and the gateway's
-- preference for sending them (`scheduled-chat-messages.sqlite3`).
-- A row is written when a schedule is accepted and kept after it ends, so a
-- second delivery of the same causal turn cannot mint another execution.
CREATE TABLE preference (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1))
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
    PRIMARY KEY (conversation, causal_execution)
) STRICT;

CREATE UNIQUE INDEX message_one_open ON message (conversation)
    WHERE status IN ('scheduled', 'delivering', 'submitting');

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
    action_id TEXT
) STRICT;

PRAGMA user_version = 1;
