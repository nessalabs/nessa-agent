PRAGMA user_version = 1;

CREATE TABLE registrations (
    id TEXT PRIMARY KEY,
    credential TEXT NOT NULL UNIQUE,
    principal TEXT NOT NULL,
    organization TEXT NOT NULL,
    owner TEXT NOT NULL,
    issued_at INTEGER NOT NULL CHECK(issued_at>=0),
    destination TEXT NOT NULL CHECK(length(destination) <= 2048),
    mac_key BLOB NOT NULL CHECK(length(mac_key) = 32),
    active INTEGER NOT NULL CHECK(active IN (0, 1)),
    stop_cause TEXT,
    stop_initiator TEXT,
    stop_at INTEGER,
    CHECK((active=1 AND stop_cause IS NULL AND stop_initiator IS NULL AND stop_at IS NULL)
       OR (active=0 AND stop_cause IS NOT NULL AND stop_at IS NOT NULL AND stop_cause IN ('credential_revoked','credential_expired') AND stop_initiator IS NOT NULL AND stop_at>=0))
);
CREATE UNIQUE INDEX active_principal ON registrations(principal) WHERE active = 1;

CREATE TABLE cursors (
    registration TEXT NOT NULL REFERENCES registrations(id),
    conversation TEXT NOT NULL,
    scope TEXT NOT NULL CHECK(length(scope) <= 4096),
    position INTEGER NOT NULL CHECK(position >= 0),
    reset_required INTEGER NOT NULL DEFAULT 0 CHECK(reset_required IN (0, 1)),
    observed_scope TEXT CHECK(observed_scope IS NULL OR length(observed_scope)<=4096),
    reset_stop TEXT CHECK(reset_stop IS NULL OR length(reset_stop)<=128),
    PRIMARY KEY(registration, conversation)
);

CREATE TABLE deliveries (
    id TEXT PRIMARY KEY,
    registration TEXT NOT NULL REFERENCES registrations(id),
    conversation TEXT NOT NULL,
    incarnation TEXT NOT NULL,
    position INTEGER NOT NULL CHECK(position > 0),
    scope TEXT NOT NULL CHECK(length(scope) <= 4096),
    body BLOB NOT NULL CHECK(length(body) <= 131072),
    source_stop TEXT CHECK(source_stop IS NULL OR length(source_stop)<=128),
    state TEXT NOT NULL CHECK(length(state) <= 512 AND json_valid(state)),
    phase TEXT GENERATED ALWAYS AS (json_extract(state, '$.phase')) STORED NOT NULL,
    due_at INTEGER GENERATED ALWAYS AS (json_extract(state, '$.due_at')) STORED NOT NULL CHECK(due_at >= 0),
    UNIQUE(registration, conversation, incarnation, position)
);
CREATE INDEX ready_deliveries ON deliveries(phase, due_at, registration, conversation, position);

CREATE TABLE audit (
    id TEXT PRIMARY KEY,
    delivery TEXT,
    registration TEXT NOT NULL,
    record TEXT NOT NULL CHECK(length(record) <= 16384),
    published INTEGER NOT NULL DEFAULT 0 CHECK(published IN (0, 1))
);

CREATE INDEX unpublished_audit ON audit(published);
CREATE INDEX delivery_audit ON audit(delivery, published);

-- Original secret-free ownership facts survive private registration retirement
-- until all its audit projections have been acknowledged and trimmed.
CREATE TABLE retirements (
    id TEXT PRIMARY KEY,
    audit TEXT NOT NULL UNIQUE REFERENCES audit(id) DEFERRABLE INITIALLY DEFERRED,
    credential TEXT NOT NULL,
    principal TEXT NOT NULL,
    organization TEXT NOT NULL,
    owner TEXT NOT NULL,
    issued_at INTEGER NOT NULL CHECK(issued_at>=0),
    stop_cause TEXT NOT NULL,
    stop_initiator TEXT NOT NULL,
    stop_at INTEGER NOT NULL CHECK(stop_at>=0),
    retired_at INTEGER NOT NULL CHECK(retired_at>=0)
);
