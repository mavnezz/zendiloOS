-- Snapshots and backups of virtual machines (zendiloos-vms,
-- docs/architecture.md, 4.13). The snapshots themselves live in ZFS, with
-- their name, schedules and consistency as properties; here is how often
-- each VM is snapshotted and how many of those are kept, as for shares.
CREATE TABLE vm_snapshot_schedules (
    vm        TEXT    NOT NULL REFERENCES vms (id) ON DELETE CASCADE,
    frequency TEXT    NOT NULL CHECK (frequency IN ('hourly', 'daily', 'weekly', 'monthly')),
    keep      INTEGER NOT NULL CHECK (keep BETWEEN 1 AND 100),
    PRIMARY KEY (vm, frequency)
) STRICT;

-- A backup as written: where (a share by its id, name and volume, or a
-- removable drive by its id), its folder beneath that root, its files
-- with their SHA-256 (a JSON array), and the VM's settings then (JSON),
-- from which it is restored. It stays when its VM goes.
CREATE TABLE vm_backups (
    id          INTEGER PRIMARY KEY,
    vm          TEXT    NOT NULL CHECK (length(vm) = 12),
    vm_name     TEXT    NOT NULL,
    place       TEXT    NOT NULL CHECK (place IN ('share', 'drive')),
    share_id    INTEGER,
    share       TEXT,
    volume      INTEGER,
    drive       TEXT,
    folder      TEXT    NOT NULL,
    taken_at    INTEGER NOT NULL,
    consistency TEXT    NOT NULL CHECK (consistency IN ('off', 'frozen', 'crash')),
    files       TEXT    NOT NULL,
    settings    TEXT    NOT NULL,
    CHECK ((place = 'share') = (share IS NOT NULL AND volume IS NOT NULL AND share_id IS NOT NULL)),
    CHECK ((place = 'drive') = (drive IS NOT NULL))
) STRICT;

CREATE INDEX vm_backups_by_vm ON vm_backups (vm, taken_at);
