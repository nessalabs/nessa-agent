-- Paired receiver identity and access epochs. This is a new private dataset;
-- the pairing use case will own who may call its transitions.
CREATE TABLE receivers (
    receiver_id TEXT PRIMARY KEY NOT NULL,
    credential_id TEXT NOT NULL UNIQUE,
    organization_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    access_epoch INTEGER NOT NULL CHECK (access_epoch > 0),
    active INTEGER NOT NULL CHECK (active IN (0, 1))
) STRICT;
CREATE TABLE receiver_transitions (
    sequence INTEGER PRIMARY KEY NOT NULL,
    receiver_id TEXT NOT NULL REFERENCES receivers(receiver_id),
    organization_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    before_epoch INTEGER,
    after_epoch INTEGER NOT NULL,
    before_active INTEGER,
    after_active INTEGER NOT NULL,
    before_credential TEXT,
    after_credential TEXT NOT NULL,
    cause TEXT NOT NULL CHECK (cause IN ('paired', 'revoked', 'regranted', 'policy_changed')),
    initiator_kind TEXT NOT NULL CHECK (initiator_kind IN ('principal', 'system')),
    initiator_id TEXT,
    request_id TEXT NOT NULL,
    observed_at_ms INTEGER NOT NULL CHECK (observed_at_ms >= 0),
    CHECK ((initiator_kind = 'principal' AND initiator_id IS NOT NULL) OR (initiator_kind = 'system' AND initiator_id IS NULL)),
    CHECK (after_epoch > 0),
    CHECK (after_active IN (0, 1))
) STRICT;
CREATE UNIQUE INDEX receiver_transition_receipt ON receiver_transitions
    (initiator_kind, ifnull(initiator_id, ''), request_id);
CREATE TABLE receiver_policy (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    revision TEXT NOT NULL
) STRICT;
PRAGMA user_version = 1;
