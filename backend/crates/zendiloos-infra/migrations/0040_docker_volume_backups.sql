-- Backups of Docker's volumes (zendiloos-containers): one plan per volume,
-- and every run with the files it wrote into its share. Retention removes
-- only files recorded here, each only while it is still the file written
-- (device and inode); restoring takes only an archive recorded here, with
-- its SHA-256.

CREATE TABLE docker_backup_plans (
    volume          TEXT    PRIMARY KEY,
    share_id        INTEGER NOT NULL,
    -- In the share, '' for its root; the archives go into <folder>/<volume>/.
    folder          TEXT    NOT NULL,
    frequency       TEXT    NOT NULL CHECK (frequency IN ('daily', 'weekly', 'monthly')),
    -- A weekly plan's weekday (0 is Monday), a monthly one's day (1 to 31).
    day             INTEGER,
    -- The host's local hour.
    hour            INTEGER NOT NULL CHECK (hour BETWEEN 0 AND 23),
    keep            INTEGER NOT NULL CHECK (keep BETWEEN 1 AND 365),
    stop_containers INTEGER NOT NULL CHECK (stop_containers IN (0, 1)),
    -- Unix seconds: runs are due from then on (made, changed or last run).
    since           INTEGER NOT NULL,
    CHECK ((frequency = 'daily') = (day IS NULL)),
    CHECK (frequency <> 'weekly' OR day BETWEEN 0 AND 6),
    CHECK (frequency <> 'monthly' OR day BETWEEN 1 AND 31)
) STRICT;

CREATE TABLE docker_backups (
    id              INTEGER PRIMARY KEY,
    volume          TEXT    NOT NULL,
    share_id        INTEGER NOT NULL,
    started_at      INTEGER NOT NULL,
    finished_at     INTEGER,
    state           TEXT    NOT NULL CHECK (state IN ('running', 'succeeded', 'failed')),
    error           TEXT,
    -- What a run that succeeded wrote, relative to the share's root, and
    -- which files they are.
    archive_path    TEXT,
    archive_bytes   INTEGER,
    archive_device  INTEGER,
    archive_inode   INTEGER,
    manifest_path   TEXT,
    manifest_bytes  INTEGER,
    manifest_device INTEGER,
    manifest_inode  INTEGER,
    sha256          TEXT,
    data_bytes      INTEGER,
    -- The manifest as written beside the archive (JSON): what a volume
    -- made again from it gets.
    manifest        TEXT,
    -- The containers stopped for it and not yet started again, a JSON
    -- array: started again should zendiloosd stop meanwhile.
    stopped         TEXT    NOT NULL DEFAULT '[]',
    -- Who started it; NULL for the schedule.
    started_by      TEXT,
    CHECK ((state = 'succeeded') = (archive_path IS NOT NULL AND sha256 IS NOT NULL))
) STRICT;

CREATE INDEX docker_backups_by_volume ON docker_backups (volume, started_at);
