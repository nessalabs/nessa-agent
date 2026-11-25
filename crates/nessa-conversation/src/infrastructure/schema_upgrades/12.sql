-- Version 12 to 13: `conversations.app_namespace`, as schema.sql declares it.
-- Existing conversations belong to the built-in default `nessa` app namespace.
ALTER TABLE conversations ADD COLUMN app_namespace TEXT NOT NULL DEFAULT 'nessa';
CREATE INDEX conversations_by_owner_namespace ON conversations (organization, owner, app_namespace);

PRAGMA user_version = 13;
