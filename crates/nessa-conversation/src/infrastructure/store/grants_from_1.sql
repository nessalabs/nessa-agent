-- Version 1 to 2: the grant log, as grants.sql creates it. Existing grant rows stay.
CREATE TABLE grant_streams (
    receiver_id TEXT PRIMARY KEY,
    incarnation TEXT NOT NULL,
    head INTEGER NOT NULL
) STRICT;
CREATE TABLE grant_log (
    receiver_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    conversation_id TEXT NOT NULL,
    resource_kind TEXT NOT NULL,
    capabilities TEXT,
    policy TEXT,
    policy_revision INTEGER,
    PRIMARY KEY (receiver_id, position)
) STRICT;
PRAGMA user_version = 2;
