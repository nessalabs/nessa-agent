-- Version 11 to 12: `owner_decisions`, as schema.sql declares it, with the
-- ask each summary waits on brought in as a decision still waiting, and an
-- empty `decision_settings`.
CREATE TABLE owner_decisions (
    conversation_id TEXT NOT NULL REFERENCES conversations (id),
    ask_id TEXT NOT NULL,
    asked_at_ms INTEGER NOT NULL,
    ask TEXT NOT NULL CHECK (length(CAST(ask AS BLOB)) BETWEEN 2 AND 8192),
    outcome TEXT CHECK (outcome IS NULL OR length(CAST(outcome AS BLOB)) BETWEEN 2 AND 8192),
    auto_pick_paused_at_ms INTEGER CHECK (auto_pick_paused_at_ms IS NULL OR auto_pick_paused_at_ms >= 0),
    PRIMARY KEY (conversation_id, ask_id)
) STRICT;
INSERT INTO owner_decisions (conversation_id, ask_id, asked_at_ms, ask, outcome)
SELECT conversation_id, json_extract(ask, '$.askId'), json_extract(ask, '$.askedAtMs'), ask, NULL
FROM summaries
WHERE ask IS NOT NULL
  AND json_valid(ask)
  AND json_type(ask, '$.askId') = 'text'
  AND json_type(ask, '$.askedAtMs') = 'integer';

CREATE TABLE decision_settings (
    organization TEXT NOT NULL,
    owner TEXT NOT NULL,
    auto_pick INTEGER NOT NULL CHECK (auto_pick IN (0, 1)),
    auto_pick_after_seconds INTEGER NOT NULL CHECK (auto_pick_after_seconds BETWEEN 60 AND 86400),
    PRIMARY KEY (organization, owner)
) STRICT, WITHOUT ROWID;

PRAGMA user_version = 12;
