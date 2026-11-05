-- How often each share is snapshotted and how many of those snapshots are
-- kept (docs/architecture.md, 5). The snapshots themselves live in ZFS.
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE share_snapshot_schedules (
    share_id  INTEGER NOT NULL REFERENCES shares (id) ON DELETE CASCADE,
    frequency TEXT    NOT NULL CHECK (frequency IN ('hourly', 'daily', 'weekly', 'monthly')),
    keep      INTEGER NOT NULL CHECK (keep BETWEEN 1 AND 100),
    PRIMARY KEY (share_id, frequency)
) STRICT;
