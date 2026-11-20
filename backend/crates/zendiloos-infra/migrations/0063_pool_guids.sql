-- The GUID of each pool zendiloOS made or imported (zendiloos-storage,
-- docs/architecture.md, 4.15), as ZFS gives it and `zpool import` lists it
-- as the pool's id: decimal text, as it is 64 bits without a sign. A
-- configuration restored on a machine whose disks hold two pools of a name
-- (the old disks of an earlier installation, another zendiloOS pool) is
-- given back the one it names, not just any pool of that name.
--
-- Filled as pools are made and imported, and at every start for the pools
-- the configuration names that are imported; a pool destroyed takes its
-- row with it. Migrations are immutable once written; change the schema
-- with a new file.
CREATE TABLE pool_guids (
    pool TEXT NOT NULL PRIMARY KEY CHECK (length(pool) BETWEEN 1 AND 255),
    guid TEXT NOT NULL CHECK (length(guid) BETWEEN 1 AND 20 AND guid NOT GLOB '*[^0-9]*')
) STRICT;
