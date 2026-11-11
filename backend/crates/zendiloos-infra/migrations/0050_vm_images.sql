-- Virtual machines made usable (zendiloos-vms, docs/architecture.md, 4.13):
-- an image library, and several disks, network cards and CD drives per VM
-- with a boot order.
--
-- The library's ISOs are files `/volume<N>/@vms/isos/<id>.iso`. The copy of
-- an ISO a VM had before was `<vm id>.iso` in that very folder: it becomes
-- an image of that id, its size looked at the first time the library is
-- listed. Its name is its file's in the share, numbered where two VMs had
-- files of the same name.
CREATE TABLE vm_images (
    -- Twelve lowercase hexadecimal digits: its file is `<id>.iso`.
    id       TEXT    PRIMARY KEY CHECK (length(id) = 12 AND id NOT GLOB '*[^0-9a-f]*'),
    name     TEXT    NOT NULL UNIQUE COLLATE NOCASE CHECK (length(name) BETWEEN 1 AND 128),
    -- NULL until known (an ISO a VM had before the library).
    bytes    INTEGER CHECK (bytes >= 0),
    -- Where it came from: a share's name and the path in it, or a browser's
    -- upload and the file's name.
    origin   TEXT    NOT NULL CHECK (origin IN ('share', 'upload')),
    share    TEXT,
    path     TEXT    NOT NULL,
    -- The volume the VMs' storage, and so its file, is on.
    volume   INTEGER NOT NULL CHECK (volume BETWEEN 1 AND 999),
    added_at INTEGER NOT NULL,
    -- 0 while it is being copied or uploaded.
    ready    INTEGER NOT NULL CHECK (ready IN (0, 1)),
    CHECK ((origin = 'share') = (share IS NOT NULL))
) STRICT;

INSERT INTO vm_images (id, name, bytes, origin, share, path, volume, added_at, ready)
SELECT id,
       substr(CASE WHEN seen = 1 THEN file ELSE file || ' (' || seen || ')' END, 1, 128),
       NULL, 'share', iso_share, iso_path, volume, created_at, 1
FROM (
    SELECT id, iso_share, iso_path, volume, created_at,
           -- The path's last part: what follows its last slash.
           replace(iso_path, rtrim(iso_path, replace(iso_path, '/', '')), '') AS file,
           row_number() OVER (
               PARTITION BY lower(replace(iso_path, rtrim(iso_path, replace(iso_path, '/', '')), ''))
               ORDER BY created_at, id
           ) AS seen
    FROM vms
    WHERE iso_share IS NOT NULL AND ready = 1
);

-- The VMs without the devices they now have several of.
CREATE TABLE vms_new (
    id         TEXT    PRIMARY KEY CHECK (length(id) = 12 AND id NOT GLOB '*[^0-9a-f]*'),
    name       TEXT    NOT NULL UNIQUE COLLATE NOCASE CHECK (length(name) BETWEEN 1 AND 48),
    os         TEXT    NOT NULL CHECK (os IN ('linux', 'windows', 'other')),
    cpus       INTEGER NOT NULL CHECK (cpus BETWEEN 1 AND 256),
    memory_mib INTEGER NOT NULL CHECK (memory_mib >= 128),
    firmware   TEXT    NOT NULL CHECK (firmware IN ('uefi', 'bios')),
    autostart  INTEGER NOT NULL CHECK (autostart IN (0, 1)),
    -- The devices it boots from, first first: `drive:0 disk:0`.
    boot       TEXT    NOT NULL,
    -- The pool its disks are on, with the VMs' storage.
    pool       TEXT    NOT NULL,
    volume     INTEGER NOT NULL CHECK (volume BETWEEN 1 AND 999),
    created_at INTEGER NOT NULL,
    -- 0 while it is being made: defined in libvirt once 1.
    ready      INTEGER NOT NULL CHECK (ready IN (0, 1))
) STRICT;

INSERT INTO vms_new (id, name, os, cpus, memory_mib, firmware, autostart, boot, pool, volume, created_at, ready)
SELECT id, name, os, cpus, memory_mib, firmware, autostart,
       -- As the VM booted: from its ISO first if it had one.
       CASE WHEN iso_share IS NOT NULL THEN 'drive:0 disk:0' ELSE 'disk:0 drive:0' END,
       pool, volume, created_at, ready
FROM vms;

-- Each disk a sparse zvol `<pool>/zendiloos.vms/<vm>-disk<slot>`.
CREATE TABLE vm_disks (
    vm       TEXT    NOT NULL REFERENCES vms_new (id) ON DELETE CASCADE,
    slot     INTEGER NOT NULL CHECK (slot BETWEEN 0 AND 7),
    size_gib INTEGER NOT NULL CHECK (size_gib BETWEEN 1 AND 65536),
    bus      TEXT    NOT NULL CHECK (bus IN ('virtio', 'sata')),
    PRIMARY KEY (vm, slot)
) STRICT;

INSERT INTO vm_disks (vm, slot, size_gib, bus)
SELECT id, 0, disk_gib, CASE os WHEN 'linux' THEN 'virtio' ELSE 'sata' END FROM vms;

CREATE TABLE vm_nics (
    vm     TEXT    NOT NULL REFERENCES vms_new (id) ON DELETE CASCADE,
    slot   INTEGER NOT NULL CHECK (slot BETWEEN 0 AND 3),
    model  TEXT    NOT NULL CHECK (model IN ('virtio', 'e1000e')),
    -- NULL: user-mode networking through passt; otherwise the bridge.
    bridge TEXT,
    -- Given once, kept whatever else changes.
    mac    TEXT    NOT NULL UNIQUE CHECK (length(mac) = 17),
    PRIMARY KEY (vm, slot)
) STRICT;

-- libvirt gave a VM's card its address when it was defined; zendiloOS
-- keeps one of its own now, from the VM's id, with QEMU's prefix.
INSERT INTO vm_nics (vm, slot, model, bridge, mac)
SELECT id, 0, CASE os WHEN 'linux' THEN 'virtio' ELSE 'e1000e' END, bridge,
       '52:54:00:' || substr(id, 7, 2) || ':' || substr(id, 9, 2) || ':' || substr(id, 11, 2)
FROM vms;

CREATE TABLE vm_drives (
    vm    TEXT    NOT NULL REFERENCES vms_new (id) ON DELETE CASCADE,
    slot  INTEGER NOT NULL CHECK (slot BETWEEN 0 AND 2),
    -- An image of the library, which stays while a drive holds it; NULL: empty.
    image TEXT    REFERENCES vm_images (id) ON DELETE RESTRICT,
    PRIMARY KEY (vm, slot)
) STRICT;

-- One drive each, with its ISO if it had one, so that an image goes in
-- while it runs.
INSERT INTO vm_drives (vm, slot, image)
SELECT v.id, 0, i.id FROM vms v LEFT JOIN vm_images i ON i.id = v.id;

DROP TABLE vms;
ALTER TABLE vms_new RENAME TO vms;

CREATE INDEX vm_drives_image ON vm_drives (image);
