-- Existing channels retain Local execution. Coordinator and workers are independent.
ALTER TABLE channels ADD COLUMN coordinator_placement TEXT NOT NULL DEFAULT 'local' CHECK (coordinator_placement IN ('local', 'boat'));
ALTER TABLE channels ADD COLUMN thread_placement TEXT NOT NULL DEFAULT 'local' CHECK (thread_placement IN ('local', 'boat'));
PRAGMA user_version = 2;
