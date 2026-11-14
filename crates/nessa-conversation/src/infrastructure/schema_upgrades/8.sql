-- Version 8 to 9: `agent_switches`, as schema.sql creates it.
CREATE TABLE agent_switches (
    conversation_id TEXT NOT NULL REFERENCES conversations (id),
    request_id TEXT NOT NULL,
    initiator TEXT NOT NULL,
    surface TEXT NOT NULL,
    prior_agent TEXT NOT NULL,
    prior_model TEXT NOT NULL,
    requested_agent TEXT NOT NULL,
    requested_model TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending','applied','not_applied')),
    requested_at_ms INTEGER NOT NULL,
    PRIMARY KEY (conversation_id, request_id),
    CHECK (prior_agent <> requested_agent)
) STRICT;
CREATE UNIQUE INDEX one_pending_agent_switch ON agent_switches (conversation_id) WHERE state = 'pending';

PRAGMA user_version = 9;
