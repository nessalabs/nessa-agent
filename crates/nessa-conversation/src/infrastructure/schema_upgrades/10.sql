-- Version 10 to 11: `summaries.ask`, as schema.sql declares it.
-- The question an agent asked the owner (`ask_owner`) and is waiting on, on
-- the conversation's summary, so the write that records the owner's answer
-- takes it down (docs/design/agent-thread-tools.md).
ALTER TABLE summaries ADD COLUMN ask TEXT CHECK (ask IS NULL OR length(CAST(ask AS BLOB)) BETWEEN 2 AND 8192);

PRAGMA user_version = 11;
