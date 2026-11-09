CREATE TABLE inbox_usage (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  deliveries INTEGER NOT NULL DEFAULT 0 CHECK (deliveries >= 0),
  bytes INTEGER NOT NULL DEFAULT 0 CHECK (bytes >= 0)
);

INSERT INTO inbox_usage (singleton) VALUES (1);

CREATE TABLE deliveries (
  delivery_id TEXT PRIMARY KEY,
  event TEXT NOT NULL,
  payload_sha256 TEXT NOT NULL,
  payload TEXT NOT NULL,
  received_at TEXT NOT NULL,
  bytes INTEGER NOT NULL CHECK (bytes >= 0)
);

CREATE TRIGGER delivery_added AFTER INSERT ON deliveries BEGIN
  UPDATE inbox_usage SET deliveries = deliveries + 1, bytes = bytes + NEW.bytes
    WHERE singleton = 1;
END;

CREATE TRIGGER delivery_removed AFTER DELETE ON deliveries BEGIN
  UPDATE inbox_usage SET deliveries = deliveries - 1, bytes = bytes - OLD.bytes
    WHERE singleton = 1;
END;
