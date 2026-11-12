-- Cloud accounts backups go to (Google Drive, OneDrive), signed in to with
-- OAuth. The token, and the administrator's own client's secret if given,
-- are never sent back; rclone's refreshed token replaces the token.
CREATE TABLE backup_cloud_accounts (
    id            INTEGER PRIMARY KEY,
    name          TEXT    NOT NULL UNIQUE COLLATE NOCASE CHECK (length(name) BETWEEN 1 AND 256),
    provider      TEXT    NOT NULL CHECK (provider IN ('google_drive', 'onedrive')),
    -- NULL: rclone's public client.
    client_id     TEXT,
    client_secret TEXT,
    token         TEXT    NOT NULL,
    -- OneDrive's drive.
    drive_id      TEXT,
    drive_type    TEXT,
    created_at    INTEGER NOT NULL
) STRICT;

-- Backup tasks (zendiloos-backup, docs/architecture.md, 4.14): a folder of
-- a share backed up to a folder on the NAS (a share, a removable drive),
-- of an rsync server or of a cloud account, or a server's folder pulled
-- into the NAS.
CREATE TABLE backup_tasks (
    id                 INTEGER PRIMARY KEY,
    name               TEXT    NOT NULL UNIQUE COLLATE NOCASE CHECK (length(name) BETWEEN 1 AND 256),
    -- The folder in the target that holds the task's backup, named after
    -- the task when it was made.
    folder             TEXT    NOT NULL CHECK (length(folder) BETWEEN 1 AND 255),
    -- Where from: a share's folder, or the server's.
    source_kind        TEXT    NOT NULL CHECK (source_kind IN ('share', 'server')),
    source_share       INTEGER,
    source_path        TEXT    NOT NULL,
    -- Where to: a share's folder, a drive's (by its file system's UUID),
    -- the server's, or a cloud account's (which stays while a task uses it).
    target_kind        TEXT    NOT NULL CHECK (target_kind IN ('share', 'drive', 'server', 'cloud')),
    target_share       INTEGER,
    target_drive       TEXT,
    target_account     INTEGER REFERENCES backup_cloud_accounts (id) ON DELETE RESTRICT,
    target_path        TEXT    NOT NULL,
    -- The server, if one end is one: an rsync daemon (its module, and a
    -- password that is never sent back) or SSH (its host key, pinned).
    server_transport   TEXT    CHECK (server_transport IN ('daemon', 'ssh')),
    server_host        TEXT,
    server_port        INTEGER CHECK (server_port BETWEEN 1 AND 65535),
    server_user        TEXT,
    server_module      TEXT,
    server_host_key    TEXT,
    server_fingerprint TEXT,
    server_password    TEXT,
    mode               TEXT    NOT NULL CHECK (mode IN ('mirror', 'versioned')),
    -- Mirror: days what a run deletes or replaces is kept; NULL: not kept.
    keep_changed_days  INTEGER,
    -- Versioned: the newest kept, and days kept; neither: all.
    keep_last          INTEGER,
    keep_days          INTEGER,
    -- NULL: by hand only.
    schedule           TEXT    CHECK (schedule IN ('hourly', 'daily', 'weekly')),
    schedule_weekday   INTEGER CHECK (schedule_weekday BETWEEN 0 AND 6),
    schedule_hour      INTEGER NOT NULL DEFAULT 0 CHECK (schedule_hour BETWEEN 0 AND 23),
    schedule_minute    INTEGER NOT NULL DEFAULT 0 CHECK (schedule_minute BETWEEN 0 AND 59),
    on_plug            INTEGER NOT NULL CHECK (on_plug IN (0, 1)),
    -- rsync's patterns, one per line.
    excludes           TEXT    NOT NULL,
    hidden             INTEGER NOT NULL CHECK (hidden IN (0, 1)),
    -- KiB/s; NULL follows the settings, 0 is none.
    bwlimit_kib        INTEGER CHECK (bwlimit_kib >= 0),
    notify_success     INTEGER NOT NULL CHECK (notify_success IN (0, 1)),
    -- With rclone's crypt: names and contents in the target encrypted with
    -- this password and salt, never sent back. Lost, the backup cannot be
    -- restored.
    encrypted          INTEGER NOT NULL DEFAULT 0 CHECK (encrypted IN (0, 1)),
    crypt_password     TEXT,
    crypt_salt         TEXT,
    created_at         INTEGER NOT NULL,
    -- Runs on its schedule are due from then: made, changed, last started.
    since              INTEGER NOT NULL,
    -- Which plugging in of its drive it last backed up for.
    last_plug          TEXT,
    CHECK ((source_kind = 'share') = (source_share IS NOT NULL)),
    CHECK ((target_kind = 'share') = (target_share IS NOT NULL)),
    CHECK ((target_kind = 'drive') = (target_drive IS NOT NULL)),
    CHECK ((target_kind = 'cloud') = (target_account IS NOT NULL)),
    CHECK ((source_kind = 'server' OR target_kind = 'server') = (server_transport IS NOT NULL)),
    CHECK (NOT (source_kind = 'server' AND target_kind = 'server'))
) STRICT;

-- The log: each run with what it transferred.
CREATE TABLE backup_runs (
    id          INTEGER PRIMARY KEY,
    task_id     INTEGER NOT NULL REFERENCES backup_tasks (id) ON DELETE CASCADE,
    cause       TEXT    NOT NULL CHECK (cause IN ('manual', 'schedule', 'plug')),
    started_by  TEXT,
    started_at  INTEGER NOT NULL,
    finished_at INTEGER,
    outcome     TEXT    NOT NULL
                CHECK (outcome IN ('running', 'succeeded', 'partial', 'failed', 'stopped', 'interrupted')),
    files       INTEGER NOT NULL DEFAULT 0,
    bytes       INTEGER NOT NULL DEFAULT 0,
    version     TEXT,
    error       TEXT,
    -- Entries of its transfer list, also those not kept.
    transfers   INTEGER NOT NULL DEFAULT 0,
    job_id      INTEGER
) STRICT;

CREATE INDEX backup_runs_by_task ON backup_runs (task_id, id);
CREATE INDEX backup_runs_by_start ON backup_runs (started_at);

-- A run's transfer list, its first 5000 entries: rsync's itemized change
-- and the path.
CREATE TABLE backup_transfers (
    run_id INTEGER NOT NULL REFERENCES backup_runs (id) ON DELETE CASCADE,
    seq    INTEGER NOT NULL,
    change TEXT    NOT NULL,
    path   TEXT    NOT NULL,
    PRIMARY KEY (run_id, seq)
) STRICT, WITHOUT ROWID;

-- The plugin's settings: one row once saved.
CREATE TABLE backup_settings (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    log_days    INTEGER NOT NULL CHECK (log_days BETWEEN 1 AND 3650),
    bwlimit_kib INTEGER NOT NULL CHECK (bwlimit_kib >= 0)
) STRICT;
