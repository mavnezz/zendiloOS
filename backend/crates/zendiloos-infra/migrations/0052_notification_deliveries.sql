-- Deliveries on notification channels that have not gone out yet, or that
-- failed for good (zendiloos-notify's queue, docs/architecture.md, 4.3):
-- one row per notification and channel, removed once it went out. A
-- notification is kept here as it was raised, also one not kept in the
-- notification center, so that it goes out after a restart of zendiloosd.
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE notification_deliveries (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    channel_id      INTEGER NOT NULL REFERENCES notification_channels (id) ON DELETE CASCADE,
    -- The notification: its ID in `notifications` (0 if it is not kept
    -- there), when it was raised, its level, code and parameters.
    notification_id INTEGER NOT NULL,
    raised_at       INTEGER NOT NULL,
    level           TEXT    NOT NULL CHECK (level IN ('info', 'success', 'warning', 'error')),
    code            TEXT    NOT NULL,
    params          TEXT    NOT NULL CHECK (json_valid(params)),
    state           TEXT    NOT NULL CHECK (state IN ('waiting', 'failed')),
    attempts        INTEGER NOT NULL CHECK (attempts >= 0),
    -- Waiting: when it is tried (again); failed: when it failed for good.
    at              INTEGER NOT NULL,
    -- Why the last try failed.
    error           TEXT
) STRICT;

CREATE INDEX notification_deliveries_channel ON notification_deliveries (channel_id, id);
