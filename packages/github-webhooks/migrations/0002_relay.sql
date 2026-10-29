CREATE TABLE relay_events (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  delivery_id TEXT NOT NULL UNIQUE REFERENCES deliveries(delivery_id) ON DELETE CASCADE,
  repository_id TEXT NOT NULL,
  installation_id TEXT NOT NULL
);

CREATE INDEX relay_repository_sequence ON relay_events(repository_id, sequence);

CREATE TABLE relay_expiry (
  repository_id TEXT PRIMARY KEY,
  through_sequence INTEGER NOT NULL
);

CREATE TABLE relay_cleanup_audit (
  cleanup_id INTEGER PRIMARY KEY AUTOINCREMENT,
  delivery_id TEXT NOT NULL,
  repository_id TEXT,
  sequence INTEGER,
  received_at TEXT NOT NULL,
  removed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  cause TEXT NOT NULL DEFAULT 'retention_expired',
  initiator TEXT NOT NULL DEFAULT 'system:relay-retention'
);

CREATE TRIGGER relay_delivery_added AFTER INSERT ON deliveries
WHEN json_type(NEW.payload, '$.repository.id') = 'integer'
 AND json_extract(NEW.payload, '$.repository.id') > 0
 AND json_type(NEW.payload, '$.installation.id') = 'integer'
 AND json_extract(NEW.payload, '$.installation.id') > 0 BEGIN
  INSERT INTO relay_events(delivery_id, repository_id, installation_id)
    VALUES(NEW.delivery_id, CAST(json_extract(NEW.payload, '$.repository.id') AS TEXT),
      CAST(json_extract(NEW.payload, '$.installation.id') AS TEXT));
END;

INSERT INTO relay_events(delivery_id, repository_id, installation_id)
SELECT delivery_id, CAST(json_extract(payload, '$.repository.id') AS TEXT),
 CAST(json_extract(payload, '$.installation.id') AS TEXT) FROM deliveries
WHERE json_type(payload, '$.repository.id') = 'integer'
 AND json_extract(payload, '$.repository.id') > 0
 AND json_type(payload, '$.installation.id') = 'integer'
 AND json_extract(payload, '$.installation.id') > 0 ORDER BY received_at, delivery_id;

CREATE TRIGGER relay_before_expiry BEFORE DELETE ON deliveries BEGIN
  INSERT INTO relay_cleanup_audit(delivery_id, repository_id, sequence, received_at)
    SELECT OLD.delivery_id, e.repository_id, e.sequence, OLD.received_at
      FROM (SELECT 1) LEFT JOIN relay_events e ON e.delivery_id = OLD.delivery_id;
  INSERT INTO relay_expiry(repository_id, through_sequence)
    SELECT repository_id, sequence FROM relay_events WHERE delivery_id = OLD.delivery_id
    ON CONFLICT(repository_id) DO UPDATE
      SET through_sequence = MAX(through_sequence, excluded.through_sequence);
END;

CREATE TABLE relay_read_audit (
  audit_id INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id TEXT NOT NULL,
  repository_id TEXT NOT NULL,
  allowed INTEGER NOT NULL,
  rules TEXT NOT NULL,
  after_sequence INTEGER NOT NULL,
  observed_at TEXT NOT NULL
);

CREATE TRIGGER relay_read_audit_bound AFTER INSERT ON relay_read_audit BEGIN
  DELETE FROM relay_read_audit WHERE audit_id <= NEW.audit_id - 10000;
END;
