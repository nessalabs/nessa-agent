CREATE TABLE installed_apps (
    app_id TEXT PRIMARY KEY NOT NULL,
    manifest BLOB NOT NULL
) STRICT;

CREATE TABLE installed_app_assets (
    app_id TEXT NOT NULL REFERENCES installed_apps(app_id),
    path TEXT NOT NULL,
    bytes BLOB NOT NULL,
    PRIMARY KEY (app_id, path)
) STRICT;

PRAGMA user_version = 1;
