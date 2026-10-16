CREATE TABLE phone_intent (
    request_id TEXT PRIMARY KEY,
    canonical BLOB NOT NULL,
    departed INTEGER NOT NULL CHECK (departed IN (0, 1)),
    outcome TEXT NOT NULL,
    failure TEXT,
    unknown TEXT,
    sends INTEGER NOT NULL CHECK (sends >= 0 AND sends <= 255),
    successor TEXT,
    CHECK (
        (departed = 0 AND outcome = 'draft' AND sends = 0
            AND failure IS NULL AND unknown IS NULL AND successor IS NULL)
        OR (departed = 1 AND outcome != 'draft')
    ),
    CHECK (
        (outcome = 'failed' AND failure IS NOT NULL AND unknown IS NULL)
        OR (outcome = 'unknown' AND unknown IS NOT NULL AND failure IS NULL)
        OR (outcome NOT IN ('failed', 'unknown') AND failure IS NULL AND unknown IS NULL)
    )
) STRICT;

CREATE TABLE phone_intent_event (
    request_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    cause TEXT NOT NULL,
    outcome TEXT NOT NULL,
    failure TEXT,
    unknown TEXT,
    initiator TEXT NOT NULL CHECK (initiator IN ('person', 'recovery')),
    observed_at_ms INTEGER NOT NULL CHECK (observed_at_ms >= 0),
    successor TEXT,
    PRIMARY KEY (request_id, ordinal),
    CHECK (
        (cause = 'returned_to_draft' AND successor IS NOT NULL)
        OR (cause != 'returned_to_draft' AND successor IS NULL)
    )
) STRICT, WITHOUT ROWID;

PRAGMA user_version = 1;
