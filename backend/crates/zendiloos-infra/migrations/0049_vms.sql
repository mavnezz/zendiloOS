-- Virtual machines (zendiloos-vms, docs/architecture.md, 4.13). libvirt
-- knows each as the domain `zendiloos-<id>`, defined from these settings;
-- the name is zendiloOS's alone and never reaches libvirt.
CREATE TABLE vms (
    -- Twelve lowercase hexadecimal digits.
    id         TEXT    PRIMARY KEY CHECK (length(id) = 12 AND id NOT GLOB '*[^0-9a-f]*'),
    name       TEXT    NOT NULL UNIQUE COLLATE NOCASE CHECK (length(name) BETWEEN 1 AND 48),
    os         TEXT    NOT NULL CHECK (os IN ('linux', 'windows', 'other')),
    cpus       INTEGER NOT NULL CHECK (cpus BETWEEN 1 AND 256),
    memory_mib INTEGER NOT NULL CHECK (memory_mib >= 128),
    firmware   TEXT    NOT NULL CHECK (firmware IN ('uefi', 'bios')),
    disk_gib   INTEGER NOT NULL CHECK (disk_gib >= 1),
    -- NULL: user-mode networking through passt; otherwise the bridge.
    bridge     TEXT,
    autostart  INTEGER NOT NULL CHECK (autostart IN (0, 1)),
    -- Where the ISO was copied from, to be shown: the share's name and the
    -- path in it. NULL: none.
    iso_share  TEXT,
    iso_path   TEXT,
    -- The pool its disk is on, with the VMs' storage.
    pool       TEXT    NOT NULL,
    volume     INTEGER NOT NULL CHECK (volume BETWEEN 1 AND 999),
    created_at INTEGER NOT NULL,
    -- 0 while it is being made: defined in libvirt once 1.
    ready      INTEGER NOT NULL CHECK (ready IN (0, 1)),
    CHECK ((iso_share IS NULL) = (iso_path IS NULL))
) STRICT;
