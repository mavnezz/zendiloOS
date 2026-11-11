-- The VMs' storage in several locations, and more kinds of network for
-- their cards (zendiloos-vms, docs/architecture.md, 4.13).
--
-- A location is the dataset `<pool>/zendiloos.vms` of a pool zendiloOS
-- made, mounted at `/volume<N>/@vms`; one per pool. The first one added
-- holds the image library. Until now there was one, on the pool every VM
-- named: it becomes the first. A location set up without a VM yet is
-- taken in by zendiloosd when it next looks at the host.
CREATE TABLE vm_locations (
    pool     TEXT    PRIMARY KEY CHECK (length(pool) BETWEEN 1 AND 255),
    volume   INTEGER NOT NULL UNIQUE CHECK (volume BETWEEN 1 AND 999),
    -- The order they were added in: the first holds the image library.
    position INTEGER NOT NULL UNIQUE
) STRICT;

INSERT INTO vm_locations (pool, volume, position)
SELECT pool, volume, row_number() OVER (ORDER BY min(created_at), pool)
FROM vms
GROUP BY pool, volume;

-- Each disk on the location it was made on: until now the VM's.
CREATE TABLE vm_disks_new (
    vm       TEXT    NOT NULL REFERENCES vms (id) ON DELETE CASCADE,
    slot     INTEGER NOT NULL CHECK (slot BETWEEN 0 AND 7),
    size_gib INTEGER NOT NULL CHECK (size_gib BETWEEN 1 AND 65536),
    bus      TEXT    NOT NULL CHECK (bus IN ('virtio', 'sata')),
    -- Its zvol is `<pool>/zendiloos.vms/<vm>-disk<slot>`.
    pool     TEXT    NOT NULL CHECK (length(pool) BETWEEN 1 AND 255),
    volume   INTEGER NOT NULL CHECK (volume BETWEEN 1 AND 999),
    PRIMARY KEY (vm, slot)
) STRICT;

INSERT INTO vm_disks_new (vm, slot, size_gib, bus, pool, volume)
SELECT d.vm, d.slot, d.size_gib, d.bus, v.pool, v.volume FROM vm_disks d JOIN vms v ON v.id = d.vm;

DROP TABLE vm_disks;
ALTER TABLE vm_disks_new RENAME TO vm_disks;

CREATE INDEX vm_disks_pool ON vm_disks (pool);

-- A card through the NAS (passt), on a bridge of the host's, directly on a
-- wired card or bond (macvtap), or on an internal network; until now
-- through the NAS, or on the bridge it named.
CREATE TABLE vm_nics_new (
    vm      TEXT    NOT NULL REFERENCES vms (id) ON DELETE CASCADE,
    slot    INTEGER NOT NULL CHECK (slot BETWEEN 0 AND 3),
    model   TEXT    NOT NULL CHECK (model IN ('virtio', 'e1000e')),
    network TEXT    NOT NULL CHECK (network IN ('user', 'bridge', 'direct', 'internal')),
    -- The bridge, card or internal network; NULL through the NAS.
    device  TEXT    CHECK (length(device) BETWEEN 1 AND 15),
    -- Given once, kept whatever else changes.
    mac     TEXT    NOT NULL UNIQUE CHECK (length(mac) = 17),
    PRIMARY KEY (vm, slot),
    CHECK ((network = 'user') = (device IS NULL))
) STRICT;

INSERT INTO vm_nics_new (vm, slot, model, network, device, mac)
SELECT vm, slot, model, CASE WHEN bridge IS NULL THEN 'user' ELSE 'bridge' END, bridge, mac FROM vm_nics;

DROP TABLE vm_nics;
ALTER TABLE vm_nics_new RENAME TO vm_nics;

-- Where its disks are is theirs now.
ALTER TABLE vms DROP COLUMN pool;
ALTER TABLE vms DROP COLUMN volume;
