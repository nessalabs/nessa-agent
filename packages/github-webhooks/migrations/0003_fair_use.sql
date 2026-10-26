ALTER TABLE deliveries ADD COLUMN installation_key TEXT GENERATED ALWAYS AS (
 CASE WHEN json_type(payload,'$.installation.id')='integer' AND json_extract(payload,'$.installation.id')>0
 THEN CAST(json_extract(payload,'$.installation.id') AS TEXT) ELSE 'unrouted' END
) VIRTUAL;

CREATE TABLE relay_installation_usage (
  installation_id TEXT PRIMARY KEY,
  deliveries INTEGER NOT NULL,
  bytes INTEGER NOT NULL
);

INSERT INTO relay_installation_usage
 SELECT installation_key,
 COUNT(*), SUM(bytes) FROM deliveries GROUP BY 1;

CREATE TRIGGER relay_installation_added AFTER INSERT ON deliveries BEGIN
 INSERT INTO relay_installation_usage VALUES (
 NEW.installation_key, 1, NEW.bytes)
 ON CONFLICT(installation_id) DO UPDATE SET deliveries=deliveries+1, bytes=bytes+excluded.bytes;
END;

CREATE TRIGGER relay_installation_removed AFTER DELETE ON deliveries BEGIN
 UPDATE relay_installation_usage SET deliveries=deliveries-1, bytes=bytes-OLD.bytes
 WHERE installation_id=OLD.installation_key;
 DELETE FROM relay_installation_usage WHERE deliveries=0;
END;

CREATE TABLE relay_user_budget (
 user_id TEXT PRIMARY KEY,
 day INTEGER NOT NULL,
 day_reads INTEGER NOT NULL,
 minute INTEGER NOT NULL,
 minute_reads INTEGER NOT NULL
);

CREATE TRIGGER relay_installation_admission BEFORE INSERT ON deliveries
WHEN NOT EXISTS(SELECT 1 FROM deliveries WHERE delivery_id=NEW.delivery_id)
 AND (COALESCE((SELECT deliveries FROM relay_installation_usage WHERE installation_id=NEW.installation_key),0)>=2500
 OR COALESCE((SELECT bytes FROM relay_installation_usage WHERE installation_id=NEW.installation_key),0)+NEW.bytes>16777216) BEGIN
 SELECT RAISE(IGNORE);
END;

ALTER TABLE deliveries ADD COLUMN repository_key TEXT GENERATED ALWAYS AS (
 CASE WHEN json_type(payload,'$.repository.id')='integer' AND json_extract(payload,'$.repository.id')>0
 THEN CAST(json_extract(payload,'$.repository.id') AS TEXT) END
) VIRTUAL;

CREATE TABLE relay_repositories (
 installation_id TEXT NOT NULL,
 repository_id TEXT NOT NULL,
 PRIMARY KEY(installation_id,repository_id)
);

INSERT OR IGNORE INTO relay_repositories
 SELECT installation_id,repository_id FROM relay_events;

INSERT OR IGNORE INTO relay_repositories
 SELECT 'expired',repository_id FROM relay_expiry;

CREATE TRIGGER relay_repository_admission BEFORE INSERT ON deliveries
WHEN NEW.repository_key IS NOT NULL AND NEW.installation_key<>'unrouted'
 AND NOT EXISTS(SELECT 1 FROM relay_repositories WHERE installation_id=NEW.installation_key AND repository_id=NEW.repository_key)
 AND ((SELECT COUNT(*) FROM relay_repositories)>=4096
 OR (SELECT COUNT(*) FROM relay_repositories WHERE installation_id=NEW.installation_key)>=256) BEGIN
 SELECT RAISE(IGNORE);
END;

CREATE TRIGGER relay_repository_added AFTER INSERT ON deliveries
WHEN NEW.repository_key IS NOT NULL AND NEW.installation_key<>'unrouted' BEGIN
 INSERT OR IGNORE INTO relay_repositories VALUES(NEW.installation_key,NEW.repository_key);
END;
