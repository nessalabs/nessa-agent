-- Whether a channel's coordinator is on (#1382 step 2). A channel made before
-- this had its main agent, so it keeps it on.
ALTER TABLE channels ADD COLUMN coordinator_on INTEGER NOT NULL DEFAULT 1 CHECK (coordinator_on IN (0, 1));
-- The job a conversation's role was told when it was created. A coordinator
-- pinned before this was told no job, so it keeps none: telling it one now
-- would change its saved prompt and lose its context.
ALTER TABLE conversation_reply_styles ADD COLUMN job TEXT CHECK (job IS NULL OR (length(job) > 0 AND length(CAST(job AS BLOB)) <= 8192));
PRAGMA user_version = 8;
