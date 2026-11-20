-- Whether a channel's coordinator is on (#1382 step 2). A channel made before
-- this had its main agent, so it keeps it on.
ALTER TABLE channels ADD COLUMN coordinator_on INTEGER NOT NULL DEFAULT 1 CHECK (coordinator_on IN (0, 1));
PRAGMA user_version = 8;
