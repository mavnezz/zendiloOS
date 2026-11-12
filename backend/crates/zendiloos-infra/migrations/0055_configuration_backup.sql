-- The configuration backup's plan (zendiloos-recovery, docs/architecture.md,
-- 4.15): one row. Backups go into a folder of a share or of a removable
-- drive, daily or weekly at a local time, the newest `keep` kept there,
-- written with the access of the administrator who set it up. The
-- passphrase is kept as the mail server's password is: zendiloosd alone
-- reads it, and it is never sent back.
CREATE TABLE configuration_backup (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    enabled     INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    frequency   TEXT    NOT NULL CHECK (frequency IN ('daily', 'weekly')),
    -- 0 is Monday; weekly only.
    weekday     INTEGER CHECK (weekday BETWEEN 0 AND 6),
    hour        INTEGER NOT NULL CHECK (hour BETWEEN 0 AND 23),
    minute      INTEGER NOT NULL CHECK (minute BETWEEN 0 AND 59),
    -- NULL: no folder chosen yet.
    target_kind TEXT    CHECK (target_kind IN ('share', 'drive')),
    -- A share that goes takes the plan's folder with it.
    share_id    INTEGER REFERENCES shares (id) ON DELETE SET NULL,
    -- A removable drive's file system UUID.
    drive       TEXT,
    folder      TEXT    NOT NULL DEFAULT '' CHECK (length(folder) <= 1024),
    keep        INTEGER NOT NULL CHECK (keep BETWEEN 1 AND 60),
    passphrase  TEXT,
    account_id  INTEGER REFERENCES accounts (id) ON DELETE SET NULL,
    -- When it was set or last started: an earlier time is not due.
    since       INTEGER NOT NULL,
    last_at     INTEGER,
    last_ok     INTEGER CHECK (last_ok IN (0, 1)),
    last_file   TEXT,
    last_error  TEXT
) STRICT;
