-- One immutable binding and current lifecycle per task. Transition audit never
-- repeats output content and commits in the same transaction as current state.
CREATE TABLE tasks (
    conversation TEXT NOT NULL,
    task TEXT NOT NULL,
    execution TEXT NOT NULL,
    tool_call TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 0),
    retained INTEGER NOT NULL CHECK (retained IN (0, 1)),
    record TEXT NOT NULL,
    PRIMARY KEY (conversation, task),
    UNIQUE (conversation, execution, tool_call)
) STRICT;
CREATE INDEX tasks_pending ON tasks (retained) WHERE retained = 1;
CREATE TABLE transitions (
    conversation TEXT NOT NULL,
    task TEXT NOT NULL,
    revision INTEGER NOT NULL,
    evidence TEXT NOT NULL,
    PRIMARY KEY (conversation, task, revision)
) STRICT;
PRAGMA user_version = 1;
