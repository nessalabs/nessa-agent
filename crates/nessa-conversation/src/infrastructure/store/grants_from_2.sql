-- Same grants database and writer as the conversation grants. Use a separate
-- projection so a directory revision cannot consume a Product conversation cursor.
CREATE TABLE directory_grant_state (
    resource_kind TEXT NOT NULL DEFAULT 'directory' CHECK (resource_kind = 'directory'),
    environment TEXT NOT NULL,
    canonical_path TEXT NOT NULL,
    subject_kind TEXT NOT NULL CHECK (subject_kind IN ('principal', 'receiver')),
    subject_id TEXT NOT NULL,
    capabilities INTEGER NOT NULL CHECK (capabilities BETWEEN 0 AND 7),
    rules TEXT,
    revision INTEGER NOT NULL CHECK (revision > 0),
    CHECK ((capabilities = 0) = (rules IS NULL)),
    PRIMARY KEY (environment, canonical_path, subject_kind, subject_id)
) STRICT;

CREATE INDEX directory_receiver_live
    ON directory_grant_state (subject_id, environment, canonical_path)
    WHERE subject_kind = 'receiver' AND capabilities <> 0;

CREATE TABLE directory_receiver_log (
    receiver_id TEXT NOT NULL,
    position INTEGER NOT NULL CHECK (position > 0),
    resource_kind TEXT NOT NULL CHECK (resource_kind = 'directory'),
    environment TEXT NOT NULL,
    canonical_path TEXT NOT NULL,
    capabilities INTEGER NOT NULL CHECK (capabilities BETWEEN 0 AND 7),
    revision INTEGER NOT NULL CHECK (revision > 0),
    PRIMARY KEY (receiver_id, position)
) STRICT;

-- Original event data stays immutable; acknowledging delivery changes only
-- the delivery flag. No external audit callback runs inside a SQLite write.
CREATE TABLE directory_grant_evidence (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    evidence_id TEXT NOT NULL UNIQUE,
    initiator TEXT NOT NULL,
    request_id TEXT NOT NULL,
    event_kind TEXT NOT NULL CHECK (event_kind IN ('set', 'revoke', 'decision')),
    environment TEXT NOT NULL,
    canonical_path TEXT NOT NULL,
    subject_kind TEXT NOT NULL CHECK (subject_kind IN ('principal', 'receiver')),
    subject_id TEXT NOT NULL,
    input TEXT NOT NULL,
    outcome TEXT NOT NULL,
    delivered INTEGER NOT NULL DEFAULT 0 CHECK (delivered IN (0, 1))
) STRICT;
CREATE UNIQUE INDEX directory_mutation_requests
    ON directory_grant_evidence (initiator, request_id) WHERE event_kind <> 'decision';
CREATE INDEX directory_grant_evidence_pending
    ON directory_grant_evidence (sequence) WHERE delivered = 0;

PRAGMA user_version = 3;
