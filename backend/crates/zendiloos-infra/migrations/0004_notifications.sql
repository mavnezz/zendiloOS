-- Notifications for administrators (docs/architecture.md, 3.1).
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE notifications (
    id     INTEGER PRIMARY KEY AUTOINCREMENT,
    at     INTEGER NOT NULL,
    level  TEXT    NOT NULL CHECK (level IN ('info', 'success', 'warning', 'error')),
    -- Stable, e.g. 'pool_trouble'; the interface words it.
    code   TEXT    NOT NULL,
    -- JSON object of string parameters, e.g. {"pool": "tank"}.
    params TEXT    NOT NULL CHECK (json_valid(params))
) STRICT;
