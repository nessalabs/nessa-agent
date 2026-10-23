PRAGMA user_version = 1;

CREATE TABLE registrations (
    id TEXT PRIMARY KEY REFERENCES registration_intents(id),
    credential TEXT NOT NULL UNIQUE,
    principal TEXT NOT NULL,
    organization TEXT NOT NULL,
    owner TEXT NOT NULL,
    issued_at INTEGER NOT NULL CHECK(issued_at>=0),
    destination TEXT NOT NULL CHECK(length(destination) <= 2048),
    mac_key BLOB NOT NULL CHECK(length(mac_key) = 32),
    active INTEGER NOT NULL CHECK(active IN (0, 1)),
    stop_cause TEXT,
    stop_initiator TEXT,
    stop_at INTEGER,
    CHECK((active=1 AND stop_cause IS NULL AND stop_initiator IS NULL AND stop_at IS NULL)
       OR (active=1 AND stop_cause='credential_expired' AND stop_initiator IS NOT NULL AND stop_at IS NOT NULL AND stop_at>=0)
       OR (active=0 AND stop_cause IS NOT NULL AND stop_at IS NOT NULL AND stop_cause IN ('credential_revoked','credential_expired') AND stop_initiator IS NOT NULL AND stop_at>=0))
);
CREATE UNIQUE INDEX active_principal ON registrations(principal) WHERE active = 1;

-- Private command inputs survive a lost auth/registration acknowledgement.
-- Their selected identity and configuration do not change on replay.
CREATE TABLE registration_intents (
    id TEXT PRIMARY KEY,
    issuer TEXT NOT NULL,
    request TEXT NOT NULL,
    issue TEXT NOT NULL CHECK(json_valid(issue)),
    destination TEXT NOT NULL,
    mac_key BLOB NOT NULL CHECK(length(mac_key) = 32),
    observed_at INTEGER NOT NULL CHECK(observed_at>=0),
    UNIQUE(issuer,request)
);
CREATE TRIGGER registration_intent_immutable BEFORE UPDATE ON registration_intents
BEGIN SELECT RAISE(ABORT,'immutable registration intent'); END;
CREATE TRIGGER registration_intent_retire AFTER DELETE ON registrations
BEGIN DELETE FROM registration_intents WHERE id=OLD.id; END;

CREATE TABLE cursors (
    registration TEXT NOT NULL REFERENCES registrations(id),
    conversation TEXT NOT NULL,
    scope TEXT NOT NULL CHECK(length(scope) <= 4096),
    position INTEGER NOT NULL CHECK(position >= 0),
    reset_required INTEGER NOT NULL DEFAULT 0 CHECK(reset_required IN (0, 1)),
    source_reset TEXT REFERENCES source_reset_facts(id),
    PRIMARY KEY(registration, conversation)
);

CREATE TABLE deliveries (
    id TEXT PRIMARY KEY,
    registration TEXT NOT NULL REFERENCES registrations(id),
    conversation TEXT NOT NULL,
    incarnation TEXT NOT NULL,
    position INTEGER NOT NULL CHECK(position > 0),
    scope TEXT NOT NULL CHECK(length(scope) <= 4096),
    body BLOB NOT NULL CHECK(length(body) <= 131072),
    state TEXT NOT NULL CHECK(length(state) <= 512 AND json_valid(state)),
    phase TEXT GENERATED ALWAYS AS (json_extract(state, '$.phase')) STORED NOT NULL,
    due_at INTEGER GENERATED ALWAYS AS (json_extract(state, '$.due_at')) STORED NOT NULL CHECK(due_at >= 0),
    UNIQUE(registration, conversation, incarnation, position)
);
CREATE INDEX ready_deliveries ON deliveries(phase, due_at, registration, conversation, position);
CREATE INDEX terminal_deliveries ON deliveries(registration) WHERE phase NOT IN ('queued','sending','retry');

CREATE TABLE audit (
    id TEXT PRIMARY KEY,
    delivery TEXT,
    registration TEXT NOT NULL,
    record TEXT NOT NULL CHECK(length(record) <= 16384),
    published INTEGER NOT NULL DEFAULT 0 CHECK(published IN (0, 1)),
    source_reset TEXT REFERENCES source_reset_facts(id)
);

CREATE INDEX unpublished_audit ON audit(published,source_reset);
CREATE INDEX delivery_audit ON audit(delivery, published);
CREATE INDEX published_registration_audit ON audit(published) WHERE delivery IS NULL;
CREATE TRIGGER delivery_audit_retire AFTER DELETE ON deliveries
WHEN OLD.phase NOT IN ('queued','sending','retry')
BEGIN DELETE FROM audit WHERE delivery=OLD.id AND published=1; END;

-- Original secret-free ownership facts survive private registration retirement
-- until all its audit projections have been acknowledged and trimmed.
CREATE TABLE retirements (
    id TEXT PRIMARY KEY,
    audit TEXT NOT NULL UNIQUE REFERENCES audit(id) DEFERRABLE INITIALLY DEFERRED,
    credential TEXT NOT NULL,
    principal TEXT NOT NULL,
    organization TEXT NOT NULL,
    owner TEXT NOT NULL,
    issued_at INTEGER NOT NULL CHECK(issued_at>=0),
    stop_cause TEXT NOT NULL,
    stop_initiator TEXT NOT NULL,
    stop_at INTEGER NOT NULL CHECK(stop_at>=0),
    retired_at INTEGER NOT NULL CHECK(retired_at>=0)
);

CREATE TABLE source_reset_facts (
    id TEXT PRIMARY KEY,
    registration TEXT NOT NULL REFERENCES registrations(id),
    fact TEXT NOT NULL,
    bytes INTEGER GENERATED ALWAYS AS (length(CAST(fact AS BLOB))) STORED,
    refs INTEGER NOT NULL DEFAULT 0 CHECK(refs>=0)
);
CREATE INDEX source_reset_registration ON source_reset_facts(registration);
CREATE TABLE source_reset_members (
    delivery TEXT PRIMARY KEY REFERENCES deliveries(id) ON DELETE CASCADE,
    reset TEXT NOT NULL REFERENCES source_reset_facts(id),
    position INTEGER NOT NULL CHECK(position>0),
    state TEXT NOT NULL
);
CREATE INDEX source_reset_members_ref ON source_reset_members(reset);
CREATE INDEX source_reset_cursor_ref ON cursors(source_reset);
CREATE INDEX source_reset_audit_ref ON audit(source_reset);
CREATE TRIGGER source_reset_fact_immutable BEFORE UPDATE OF id,registration,fact ON source_reset_facts
BEGIN SELECT RAISE(ABORT,'immutable source reset'); END;
CREATE TRIGGER source_reset_member_immutable BEFORE UPDATE ON source_reset_members
BEGIN SELECT RAISE(ABORT,'immutable source capture'); END;
CREATE TRIGGER source_reset_member_retention BEFORE DELETE ON source_reset_members
WHEN EXISTS(SELECT 1 FROM deliveries WHERE id=OLD.delivery)
BEGIN SELECT RAISE(ABORT,'source capture still owned'); END;
CREATE TRIGGER source_reset_last_reference AFTER UPDATE OF refs ON source_reset_facts
WHEN NEW.refs=0
BEGIN DELETE FROM source_reset_facts WHERE id=NEW.id; END;
CREATE TRIGGER source_reset_cursor_retain AFTER INSERT ON cursors
WHEN NEW.source_reset IS NOT NULL
BEGIN UPDATE source_reset_facts SET refs=refs+1 WHERE id=NEW.source_reset; END;
CREATE TRIGGER source_reset_cursor_release AFTER DELETE ON cursors
WHEN OLD.source_reset IS NOT NULL
BEGIN UPDATE source_reset_facts SET refs=refs-1 WHERE id=OLD.source_reset; END;
CREATE TRIGGER source_reset_cursor_replace AFTER UPDATE OF source_reset ON cursors
WHEN OLD.source_reset IS NOT NEW.source_reset
BEGIN
UPDATE source_reset_facts SET refs=refs-1 WHERE id=OLD.source_reset;
UPDATE source_reset_facts SET refs=refs+1 WHERE id=NEW.source_reset;
END;
CREATE TRIGGER source_reset_member_retain AFTER INSERT ON source_reset_members
WHEN NEW.reset IS NOT NULL
BEGIN UPDATE source_reset_facts SET refs=refs+1 WHERE id=NEW.reset; END;
CREATE TRIGGER source_reset_member_release AFTER DELETE ON source_reset_members
WHEN OLD.reset IS NOT NULL
BEGIN UPDATE source_reset_facts SET refs=refs-1 WHERE id=OLD.reset; END;
CREATE TRIGGER source_reset_audit_retain AFTER INSERT ON audit
WHEN NEW.source_reset IS NOT NULL
BEGIN UPDATE source_reset_facts SET refs=refs+1 WHERE id=NEW.source_reset; END;
CREATE TRIGGER source_reset_audit_release AFTER DELETE ON audit
WHEN OLD.source_reset IS NOT NULL
BEGIN UPDATE source_reset_facts SET refs=refs-1 WHERE id=OLD.source_reset; END;
CREATE TRIGGER source_reset_audit_replace AFTER UPDATE OF source_reset ON audit
WHEN OLD.source_reset IS NOT NEW.source_reset
BEGIN
UPDATE source_reset_facts SET refs=refs-1 WHERE id=OLD.source_reset;
UPDATE source_reset_facts SET refs=refs+1 WHERE id=NEW.source_reset;
END;
