-- Long-running operations (docs/architecture.md, 3.1).
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE jobs (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Dotted name, e.g. 'storage.create_pool'.
    kind         TEXT    NOT NULL,
    target       TEXT,
    state        TEXT    NOT NULL CHECK (state IN ('running', 'succeeded', 'failed', 'interrupted')),
    step         TEXT,
    progress     REAL    CHECK (progress IS NULL OR (progress >= 0 AND progress <= 1)),
    error_code   TEXT,
    error_detail TEXT,
    -- As in audit_log: no foreign key, the history outlives accounts.
    account_id   INTEGER,
    actor        TEXT,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL,
    finished_at  INTEGER
) STRICT;

CREATE INDEX jobs_state ON jobs (state);
