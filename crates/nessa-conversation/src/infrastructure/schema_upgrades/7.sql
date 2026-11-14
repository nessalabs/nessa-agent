-- Version 7 to 8: `model_requests`, as schema.sql creates it. #975 added
-- the table without raising the version, so a version 7 file may hold it
-- already, written by the same definition, or not: `IF NOT EXISTS` covers
-- both, and the store then checks the table it finds (`validate_shape`).
CREATE TABLE IF NOT EXISTS model_requests (
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
CREATE UNIQUE INDEX IF NOT EXISTS one_pending_model_request ON model_requests(conversation_id) WHERE state='pending';

PRAGMA user_version = 8;
