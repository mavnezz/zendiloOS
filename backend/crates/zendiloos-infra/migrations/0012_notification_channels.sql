-- Channels that carry notifications beyond the interface: email and
-- webhooks (docs/architecture.md, 4.3). The settings are JSON, the mail
-- password among them: the database is readable by zendiloosd only.
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE notification_channels (
    id         INTEGER PRIMARY KEY,
    name       TEXT    NOT NULL UNIQUE,
    kind       TEXT    NOT NULL CHECK (kind IN ('email', 'webhook')),
    settings   TEXT    NOT NULL,
    min_level  TEXT    NOT NULL CHECK (min_level IN ('info', 'warning', 'error')),
    language   TEXT    NOT NULL CHECK (language IN ('en', 'de')),
    -- The last delivery: when, and what went wrong (NULL: nothing).
    last_at    INTEGER,
    last_error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;
