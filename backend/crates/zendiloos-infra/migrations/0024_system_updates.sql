-- Bounded context Updates: how the NAS looks for updates, what it found
-- and what it said (docs/architecture.md, 4.10). One settings row at most;
-- without it, the defaults: the stable channel, looked at daily.
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE system_update_settings (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    channel     TEXT    NOT NULL CHECK (channel IN ('stable', 'test', 'development')),
    check_daily INTEGER NOT NULL CHECK (check_daily IN (0, 1))
) STRICT;

-- `check`: the last check as JSON; `said:<topic>`: the key of what was
-- last said about it, to say each finding once.
CREATE TABLE system_update_state (
    topic TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;
