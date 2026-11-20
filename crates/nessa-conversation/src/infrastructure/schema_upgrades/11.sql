-- Version 11 to 12: `owner_decisions`, as schema.sql declares it, with the
-- ask each summary waits on brought in as a decision still waiting.
CREATE TABLE owner_decisions (
    conversation_id TEXT NOT NULL REFERENCES conversations (id),
    ask_id TEXT NOT NULL,
    asked_at_ms INTEGER NOT NULL,
    ask TEXT NOT NULL CHECK (length(CAST(ask AS BLOB)) BETWEEN 2 AND 8192),
    outcome TEXT CHECK (outcome IS NULL OR length(CAST(outcome AS BLOB)) BETWEEN 2 AND 4096),
    PRIMARY KEY (conversation_id, ask_id)
) STRICT;
INSERT INTO owner_decisions (conversation_id, ask_id, asked_at_ms, ask, outcome)
SELECT conversation_id, json_extract(ask, '$.askId'), json_extract(ask, '$.askedAtMs'), ask, NULL
FROM summaries
WHERE ask IS NOT NULL
  AND json_valid(ask)
  AND json_type(ask, '$.askId') = 'text'
  AND json_type(ask, '$.askedAtMs') = 'integer';

PRAGMA user_version = 12;
