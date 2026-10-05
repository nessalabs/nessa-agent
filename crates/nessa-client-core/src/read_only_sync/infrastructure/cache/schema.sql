CREATE TABLE transcript_progress (
    receiver TEXT NOT NULL,
    origin TEXT NOT NULL,
    stream TEXT NOT NULL,
    incarnation TEXT NOT NULL,
    schema_id TEXT NOT NULL,
    access_epoch TEXT NOT NULL,
    downloaded BLOB NOT NULL CHECK (length(downloaded) = 8),
    applied BLOB NOT NULL CHECK (length(applied) = 8 AND applied <= downloaded),
    facts BLOB NOT NULL CHECK (length(facts) = 8),
    generation BLOB NOT NULL CHECK (length(generation) = 8),
    PRIMARY KEY (receiver, origin, stream)
) STRICT, WITHOUT ROWID;
CREATE TABLE transcript_records (
    receiver TEXT NOT NULL,
    origin TEXT NOT NULL,
    stream TEXT NOT NULL,
    position BLOB NOT NULL CHECK (length(position) = 8),
    record_id TEXT NOT NULL,
    payload BLOB NOT NULL,
    PRIMARY KEY (receiver, origin, stream, position),
    UNIQUE (receiver, origin, stream, record_id),
    FOREIGN KEY (receiver, origin, stream)
        REFERENCES transcript_progress (receiver, origin, stream)
) STRICT, WITHOUT ROWID;
CREATE TABLE transcript_checkpoints (
    receiver TEXT NOT NULL,
    origin TEXT NOT NULL,
    stream TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    payload BLOB NOT NULL,
    PRIMARY KEY (receiver, origin, stream, ordinal),
    FOREIGN KEY (receiver, origin, stream)
        REFERENCES transcript_progress (receiver, origin, stream)
) STRICT, WITHOUT ROWID;
CREATE TABLE cache_resets (
    operation TEXT PRIMARY KEY,
    caller TEXT NOT NULL,
    receiver TEXT NOT NULL,
    origin TEXT NOT NULL,
    stream TEXT NOT NULL,
    old_incarnation TEXT NOT NULL,
    old_schema TEXT NOT NULL,
    old_epoch TEXT NOT NULL,
    new_incarnation TEXT NOT NULL,
    new_schema TEXT NOT NULL,
    new_epoch TEXT NOT NULL,
    before_generation BLOB NOT NULL CHECK(length(before_generation)=8),
    before_downloaded BLOB NOT NULL CHECK(length(before_downloaded)=8),
    before_applied BLOB NOT NULL CHECK(length(before_applied)=8),
    before_facts BLOB NOT NULL CHECK(length(before_facts)=8),
    after_generation BLOB NOT NULL CHECK(length(after_generation)=8),
    observed_at_ms BLOB NOT NULL CHECK(length(observed_at_ms)=8),
    cause TEXT NOT NULL CHECK(cause='ExplicitReset'),
    initiator TEXT NOT NULL CHECK(initiator='LocalOperator')
) STRICT, WITHOUT ROWID;
CREATE TABLE catalogue_progress (
    receiver TEXT NOT NULL, origin TEXT NOT NULL, stream TEXT NOT NULL,
    incarnation TEXT NOT NULL, schema_id TEXT NOT NULL, access_epoch TEXT NOT NULL,
    completed BLOB NOT NULL CHECK(length(completed)=8),
    generation BLOB NOT NULL CHECK(length(generation)=8),
    active_boundary BLOB CHECK(active_boundary IS NULL OR length(active_boundary)=8),
    cursor_creation BLOB CHECK(cursor_creation IS NULL OR length(cursor_creation)=8),
    cursor_id TEXT,
    PRIMARY KEY(receiver,origin,stream)
) STRICT, WITHOUT ROWID;
CREATE TABLE catalogue_entries (
    receiver TEXT NOT NULL, origin TEXT NOT NULL, stream TEXT NOT NULL,
    entry_id TEXT NOT NULL,
    creation BLOB NOT NULL CHECK(length(creation)=8),
    revision BLOB NOT NULL CHECK(length(revision)=8),
    deleted INTEGER NOT NULL CHECK(deleted IN (0,1)),
    payload BLOB NOT NULL,
    PRIMARY KEY(receiver,origin,stream,entry_id),
    FOREIGN KEY(receiver,origin,stream) REFERENCES catalogue_progress(receiver,origin,stream)
) STRICT, WITHOUT ROWID;
CREATE INDEX catalogue_entry_order ON catalogue_entries(receiver,origin,stream,creation,entry_id);
CREATE TABLE catalogue_deletions (
    receiver TEXT NOT NULL, origin TEXT NOT NULL, conversation TEXT NOT NULL,
    catalogue_stream TEXT NOT NULL, catalogue_incarnation TEXT NOT NULL,
    catalogue_schema TEXT NOT NULL, access_epoch TEXT NOT NULL,
    creation BLOB NOT NULL CHECK(length(creation)=8),
    revision BLOB NOT NULL CHECK(length(revision)=8),
    before_revision BLOB CHECK(before_revision IS NULL OR length(before_revision)=8),
    before_deleted INTEGER CHECK(before_deleted IS NULL OR before_deleted IN (0,1)),
    transcript_incarnation TEXT, transcript_schema TEXT, transcript_epoch TEXT,
    transcript_facts BLOB CHECK(transcript_facts IS NULL OR length(transcript_facts)=8),
    transcript_downloaded BLOB CHECK(transcript_downloaded IS NULL OR length(transcript_downloaded)=8),
    transcript_applied BLOB CHECK(transcript_applied IS NULL OR length(transcript_applied)=8),
    transcript_generation BLOB CHECK(transcript_generation IS NULL OR length(transcript_generation)=8),
    observed_at_ms BLOB NOT NULL CHECK(length(observed_at_ms)=8),
    cause TEXT NOT NULL CHECK(cause='SourceDeletion'),
    initiator TEXT NOT NULL CHECK(initiator='RemoteCatalogue'),
    PRIMARY KEY(receiver,origin,conversation)
) STRICT, WITHOUT ROWID;
CREATE TABLE catalogue_resets (
    operation TEXT PRIMARY KEY, caller TEXT NOT NULL,
    receiver TEXT NOT NULL, origin TEXT NOT NULL, stream TEXT NOT NULL,
    old_incarnation TEXT NOT NULL, old_schema TEXT NOT NULL, old_epoch TEXT NOT NULL,
    new_incarnation TEXT NOT NULL, new_schema TEXT NOT NULL, new_epoch TEXT NOT NULL,
    before_generation BLOB NOT NULL CHECK(length(before_generation)=8),
    before_completed BLOB NOT NULL CHECK(length(before_completed)=8),
    before_boundary BLOB CHECK(before_boundary IS NULL OR length(before_boundary)=8),
    before_cursor_creation BLOB CHECK(before_cursor_creation IS NULL OR length(before_cursor_creation)=8),
    before_cursor_id TEXT,
    after_generation BLOB NOT NULL CHECK(length(after_generation)=8),
    observed_at_ms BLOB NOT NULL CHECK(length(observed_at_ms)=8),
    cause TEXT NOT NULL CHECK(cause='ExplicitReset'),
    initiator TEXT NOT NULL CHECK(initiator='LocalOperator')
) STRICT, WITHOUT ROWID;
-- An authenticated Terminal enrollment status ended this receiver: its rows
-- were deleted in the same transaction, and this receipt is the fence that
-- refuses any later apply for it.
CREATE TABLE cache_purges (
    receiver TEXT PRIMARY KEY,
    cause TEXT NOT NULL CHECK(cause='TerminalEnrollment'),
    initiator TEXT NOT NULL CHECK(initiator='GatewayStatus'),
    transcripts BLOB NOT NULL CHECK(length(transcripts)=8),
    records BLOB NOT NULL CHECK(length(records)=8),
    catalogue_entries BLOB NOT NULL CHECK(length(catalogue_entries)=8),
    observed_at_ms BLOB NOT NULL CHECK(length(observed_at_ms)=8)
) STRICT, WITHOUT ROWID;
PRAGMA user_version = 1;
