-- Frozen version-6 scoped heads preserve saved catalogue cursors (#724).
-- New databases start empty; only the explicit version-6 upgrade seeds these.
CREATE TABLE catalogue_reader_floors (
    organization TEXT NOT NULL,
    owner TEXT NOT NULL,
    receiver TEXT NOT NULL,
    head INTEGER NOT NULL CHECK (head >= 0),
    PRIMARY KEY (organization, owner, receiver),
    FOREIGN KEY (organization, owner) REFERENCES catalogue_owners (organization, owner)
) STRICT;

-- Immediate-predecessor lookups stay within one resource/principal history.
CREATE INDEX share_grant_changes_by_principal
    ON share_grant_changes (resource_kind, resource_id, principal_id, sequence);

PRAGMA user_version = 7;
