CREATE TABLE bindings (
  generation TEXT NOT NULL,
  user_id TEXT NOT NULL,
  repository_id TEXT NOT NULL,
  repository_name TEXT NOT NULL,
  cursor INTEGER NOT NULL DEFAULT 0,
  gap_through INTEGER,
  PRIMARY KEY(user_id, repository_id)
);
CREATE TABLE events (
  user_id TEXT NOT NULL,
  repository_id TEXT NOT NULL,
  sequence INTEGER NOT NULL,
  delivery_id TEXT NOT NULL,
  event TEXT NOT NULL,
  payload TEXT NOT NULL,
  received_at TEXT NOT NULL,
  stored_at INTEGER NOT NULL,
  PRIMARY KEY(user_id, repository_id, delivery_id),
  UNIQUE(user_id, repository_id, sequence),
  FOREIGN KEY(user_id, repository_id) REFERENCES bindings(user_id, repository_id)
);
CREATE TABLE audit (
  id INTEGER PRIMARY KEY,
  user_id TEXT NOT NULL,
  repository_id TEXT NOT NULL,
  delivery_id TEXT,
  before_cursor INTEGER,
  after_cursor INTEGER,
  cause TEXT NOT NULL,
  initiator TEXT NOT NULL,
  observed_at INTEGER NOT NULL
);
PRAGMA user_version = 1;

CREATE TABLE binding_audit (
  id INTEGER PRIMARY KEY,
  user_id TEXT NOT NULL,
  repository_id TEXT NOT NULL,
  before_generation TEXT,
  before_cursor INTEGER,
  after_cursor INTEGER,
  after_generation TEXT,
  cause TEXT NOT NULL,
  initiator TEXT NOT NULL,
  observed_at INTEGER NOT NULL
);
